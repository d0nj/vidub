use serde_json::Value;
use tokio::io::{split, AsyncBufReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};

use crate::transport::IpcStream;

pub fn seconds_to_ms(value: &Value) -> Option<u64> {
    let seconds = value.as_f64()?;
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    Some((seconds * 1000.0).round() as u64)
}

pub struct IpcCodec<S> {
    reader: BufReader<ReadHalf<S>>,
    writer: WriteHalf<S>,
    next_id: u64,
}

impl<S: IpcStream> IpcCodec<S> {
    pub fn new(stream: S) -> Self {
        let (read, write) = split(stream);
        Self {
            reader: BufReader::new(read),
            writer: write,
            next_id: 1,
        }
    }

    pub async fn send(&mut self, command: &[Value]) -> anyhow::Result<Value> {
        let id = self.next_id;
        self.next_id = if self.next_id == u64::MAX {
            1
        } else {
            self.next_id + 1
        };
        self.write_command(id, command).await?;
        loop {
            let Some(message) = self.read_json().await else {
                anyhow::bail!("mpv IPC closed while awaiting reply to command {id}");
            };
            if message.get("request_id").and_then(Value::as_u64) == Some(id) {
                return reply_data(&message);
            }
        }
    }

    async fn read_json(&mut self) -> Option<Value> {
        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line).await {
                Ok(0) => return None,
                Ok(_) => {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    if let Ok(value) = serde_json::from_str(line) {
                        return Some(value);
                    }
                }
                Err(_) => return None,
            }
        }
    }

    async fn write_command(&mut self, id: u64, command: &[Value]) -> anyhow::Result<()> {
        let request = serde_json::json!({"command": command, "request_id": id});
        let mut bytes = serde_json::to_vec(&request)?;
        bytes.push(b'\n');
        self.writer.write_all(&bytes).await?;
        self.writer.flush().await?;
        Ok(())
    }
}

fn reply_data(reply: &Value) -> anyhow::Result<Value> {
    if let Some(error) = reply.get("error").and_then(Value::as_str) {
        if error != "success" {
            return Err(anyhow::anyhow!("mpv IPC error: {error}"));
        }
    }
    Ok(reply.get("data").cloned().unwrap_or(Value::Null))
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};
    use tokio::io::{
        duplex, AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf,
    };

    use super::*;

    type Server = (BufReader<ReadHalf<DuplexStream>>, WriteHalf<DuplexStream>);

    fn pair() -> (IpcCodec<DuplexStream>, Server) {
        let (a, b) = duplex(4096);
        let (a_read, a_write) = split(a);
        (IpcCodec::new(b), (BufReader::new(a_read), a_write))
    }

    async fn read_request(reader: &mut BufReader<ReadHalf<DuplexStream>>) -> Value {
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        serde_json::from_str(&line).unwrap()
    }

    #[tokio::test]
    async fn send_returns_matching_reply_data() {
        let (mut codec, (mut server, mut server_write)) = pair();
        let server = tokio::spawn(async move {
            let request = read_request(&mut server).await;
            assert_eq!(request["command"], json!(["get_property", "time-pos"]));
            let id = request["request_id"].as_u64().unwrap();
            server_write
                .write_all(b"{\"event\":\"property-change\",\"id\":1001,\"name\":\"time-pos\",\"data\":1.5}\n")
                .await
                .unwrap();
            server_write
                .write_all(
                    format!("{{\"error\":\"success\",\"data\":2.5,\"request_id\":{id}}}\n")
                        .as_bytes(),
                )
                .await
                .unwrap();
            server_write.flush().await.unwrap();
        });
        let data = codec
            .send(&[json!("get_property"), json!("time-pos")])
            .await
            .unwrap();
        assert_eq!(data, Value::from(2.5));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn send_assigns_incrementing_request_ids() {
        let (mut codec, (mut server, mut server_write)) = pair();
        let server = tokio::spawn(async move {
            let mut ids = Vec::new();
            for _ in 0..2 {
                let request = read_request(&mut server).await;
                let id = request["request_id"].as_u64().unwrap();
                ids.push(id);
                server_write
                    .write_all(
                        format!("{{\"error\":\"success\",\"data\":null,\"request_id\":{id}}}\n")
                            .as_bytes(),
                    )
                    .await
                    .unwrap();
                server_write.flush().await.unwrap();
            }
            ids
        });
        codec
            .send(&[json!("get_property"), json!("duration")])
            .await
            .unwrap();
        codec
            .send(&[json!("get_property"), json!("time-pos")])
            .await
            .unwrap();
        assert_eq!(server.await.unwrap(), vec![1, 2]);
    }

    #[tokio::test]
    async fn send_bails_on_mpv_error() {
        let (mut codec, (mut server, mut server_write)) = pair();
        let server = tokio::spawn(async move {
            let request = read_request(&mut server).await;
            let id = request["request_id"].as_u64().unwrap();
            server_write
                .write_all(
                    format!("{{\"error\":\"property unavailable\",\"request_id\":{id}}}\n")
                        .as_bytes(),
                )
                .await
                .unwrap();
            server_write.flush().await.unwrap();
        });
        let err = codec
            .send(&[json!("get_property"), json!("duration")])
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("property unavailable"),
            "unexpected error: {err}"
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn malformed_lines_are_skipped() {
        let (mut codec, (mut server, mut server_write)) = pair();
        let server = tokio::spawn(async move {
            let request = read_request(&mut server).await;
            let id = request["request_id"].as_u64().unwrap();
            server_write
                .write_all(
                    format!(
                        "garbage\n\n{{\"not json\"\n{{\"error\":\"success\",\"data\":7,\"request_id\":{id}}}\n"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            server_write.flush().await.unwrap();
        });
        let data = codec
            .send(&[json!("get_property"), json!("duration")])
            .await
            .unwrap();
        assert_eq!(data, Value::from(7));
        server.await.unwrap();
    }

    #[test]
    fn seconds_to_ms_cases() {
        assert_eq!(seconds_to_ms(&json!(1.5)), Some(1500));
        assert_eq!(seconds_to_ms(&json!(0)), Some(0));
        assert_eq!(seconds_to_ms(&json!(12.345)), Some(12345));
        assert_eq!(seconds_to_ms(&Value::Null), None);
        assert_eq!(seconds_to_ms(&json!(-1.0)), None);
        assert_eq!(seconds_to_ms(&json!("12")), None);
        assert_eq!(seconds_to_ms(&json!(true)), None);
    }
}

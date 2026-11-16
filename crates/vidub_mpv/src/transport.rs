use std::path::PathBuf;

#[cfg(any(unix, test))]
use std::path::Path;

use tokio::io::{AsyncRead, AsyncWrite};

pub const WINDOWS_PIPE_NAME: &str = r"\\.\pipe\vidub-mpv";

#[cfg(any(unix, test))]
const UNIX_SOCKET_FILE: &str = "vidub-mpv.sock";

#[derive(Debug, Clone)]
pub enum MpvTransport {
    WindowsPipe { pipe: String },
    UnixSocket { path: PathBuf },
}

impl MpvTransport {
    pub fn for_current_platform() -> Self {
        #[cfg(windows)]
        {
            Self::WindowsPipe {
                pipe: WINDOWS_PIPE_NAME.to_string(),
            }
        }
        #[cfg(not(windows))]
        {
            let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .filter(|p| !p.as_os_str().is_empty());
            Self::UnixSocket {
                path: unix_socket_path(
                    runtime_dir.as_deref(),
                    &std::env::temp_dir(),
                    std::process::id(),
                ),
            }
        }
    }

    pub fn ipc_server_arg(&self) -> String {
        match self {
            Self::WindowsPipe { pipe } => format!("--input-ipc-server={pipe}"),
            Self::UnixSocket { path } => format!("--input-ipc-server={}", path.display()),
        }
    }
}

#[cfg(any(unix, test))]
fn unix_socket_path(runtime_dir: Option<&Path>, temp_dir: &Path, pid: u32) -> PathBuf {
    if let Some(dir) = runtime_dir.filter(|p| !p.as_os_str().is_empty()) {
        return dir.join(UNIX_SOCKET_FILE);
    }
    temp_dir.join(format!("vidub-mpv-{pid}.sock"))
}

pub trait IpcStream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T> IpcStream for T where T: AsyncRead + AsyncWrite + Unpin + Send {}

pub async fn connect(transport: &MpvTransport) -> anyhow::Result<Box<dyn IpcStream>> {
    match transport {
        #[cfg(windows)]
        MpvTransport::WindowsPipe { pipe } => {
            let client = tokio::net::windows::named_pipe::ClientOptions::new().open(pipe)?;
            Ok(Box::new(client))
        }
        #[cfg(unix)]
        MpvTransport::UnixSocket { path } => {
            let stream = tokio::net::UnixStream::connect(path).await?;
            Ok(Box::new(stream))
        }
        _ => anyhow::bail!("unsupported mpv transport for this platform"),
    }
}

pub fn cleanup(transport: &MpvTransport) {
    if let MpvTransport::UnixSocket { path } = transport {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn ipc_server_arg_shape() {
        let pipe = MpvTransport::WindowsPipe {
            pipe: "\\\\.\\pipe\\vidub-mpv".to_string(),
        };
        assert_eq!(
            pipe.ipc_server_arg(),
            r"--input-ipc-server=\\.\pipe\vidub-mpv"
        );
        let sock = MpvTransport::UnixSocket {
            path: PathBuf::from("/tmp/vidub-mpv-7.sock"),
        };
        assert_eq!(
            sock.ipc_server_arg(),
            "--input-ipc-server=/tmp/vidub-mpv-7.sock"
        );
    }

    #[cfg(windows)]
    #[test]
    fn for_current_platform_is_named_pipe() {
        match MpvTransport::for_current_platform() {
            MpvTransport::WindowsPipe { pipe } => {
                assert_eq!(pipe, WINDOWS_PIPE_NAME);
            }
            other => panic!("expected WindowsPipe, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn for_current_platform_is_unix_socket() {
        match MpvTransport::for_current_platform() {
            MpvTransport::UnixSocket { path } => {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                assert!(
                    name.contains("vidub-mpv") && name.ends_with(".sock"),
                    "got {path:?}"
                );
            }
            other => panic!("expected UnixSocket, got {other:?}"),
        }
    }

    #[test]
    fn unix_socket_path_prefers_runtime_dir() {
        let path = unix_socket_path(Some(Path::new("/run/user/1000")), Path::new("/tmp"), 4242);
        assert_eq!(path, Path::new("/run/user/1000/vidub-mpv.sock"));
    }

    #[test]
    fn unix_socket_path_tmp_fallback() {
        assert_eq!(
            unix_socket_path(None, Path::new("/tmp"), 7),
            Path::new("/tmp/vidub-mpv-7.sock")
        );
        assert_eq!(
            unix_socket_path(Some(Path::new("")), Path::new("/tmp"), 7),
            Path::new("/tmp/vidub-mpv-7.sock")
        );
    }

    #[test]
    fn cleanup_unlinks_unix_socket() {
        let path = std::env::temp_dir().join(format!(
            "vidub-mpv-cleanup-test-{}-{}.sock",
            std::process::id(),
            "test"
        ));
        std::fs::write(&path, b"stale").unwrap();
        assert!(path.exists());
        cleanup(&MpvTransport::UnixSocket { path: path.clone() });
        assert!(!path.exists());
        cleanup(&MpvTransport::WindowsPipe {
            pipe: WINDOWS_PIPE_NAME.to_string(),
        });
    }

    #[tokio::test]
    async fn connect_to_absent_endpoint_errors() {
        #[cfg(windows)]
        {
            let transport = MpvTransport::WindowsPipe {
                pipe: r"\\.\pipe\vidub-mpv-test-absent".to_string(),
            };
            assert!(connect(&transport).await.is_err());
        }
        #[cfg(not(windows))]
        {
            let transport = MpvTransport::UnixSocket {
                path: std::env::temp_dir().join("vidub-mpv-test-absent.sock"),
            };
            assert!(connect(&transport).await.is_err());
        }
    }
}

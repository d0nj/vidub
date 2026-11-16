[CmdletBinding(SupportsShouldProcess = $true)]
param(
  [string]$DistDir = ""
)
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$IsDryRun = $WhatIfPreference
if (-not $DistDir) { $DistDir = (Join-Path (Join-Path $PSScriptRoot "..") "dist") }
$DistDir  = (Resolve-Path -LiteralPath $DistDir -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Path)
if (-not $DistDir) { $DistDir = (Join-Path (Join-Path $PSScriptRoot "..") "dist") }
$BinDir    = Join-Path $DistDir "bin"
$ModelsDir = Join-Path $DistDir "models"
$StateDir  = Join-Path $DistDir ".fetch-state"
$CacheDir  = Join-Path ([IO.Path]::GetTempPath()) "vidub-fetch"
$ExtRoot   = Join-Path $CacheDir "extracted"

function Get-FileSha256([string]$Path) {
  $out = & certutil -hashfile $Path SHA256 2>$null
  foreach ($line in $out) {
    $t = $line.Trim().Replace(" ", "")
    if ($t -match '^[0-9a-fA-F]{64}$') { return $t.ToLowerInvariant() }
  }
  throw "SHA-256 failed for $Path"
}

function New-JsonEntry($a, [string]$StagedHash) {
  return [pscustomobject]@{
    name       = [IO.Path]::GetFileName($a.Dest)
    version    = $a.Version
    sha256     = $StagedHash
    source_url = $a.Url
  }
}

$Artifacts = @(
  @{ Name = "yt-dlp"; Version = "2026.10.26"; Kind = "file"
     Pin = "f5389d3698ecde4ef06f4e6b1ce0294fe9ac1396228c8990b8d3416d6d177a7d"
     Url  = "https://github.com/yt-dlp/yt-dlp/releases/download/2026.10.26/yt-dlp.exe"
     Dest = "bin/yt-dlp.exe" },
  @{ Name = "mpv"; Version = "v0.42.0"; Kind = "zip-dir"
     Pin = "8dd69cbab079086765fa5d873c09513b2834b85d146c1301718a757e8c0595ef"
     Url  = "https://github.com/mpv-player/mpv/releases/download/v0.42.0/mpv-v0.42.0-x86_64-pc-windows-msvc.zip"
     Dest = "bin/mpv.exe"; ExeName = "mpv.exe" },
  @{ Name = "ffmpeg"; Version = "N-128728-g49a3b1f259"; Kind = "zip-dir"
     Pin = "acb67f124da510d346c0f2afe19face6f316f0d8959b595ad5050b1b86bfbcbc"
     Url  = "https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/download/autobuild-2026-11-10-12-39/ffmpeg-N-128728-g49a3b1f259-win64-lgpl-shared.zip"
     Dest = "bin/ffmpeg.exe"; ExeName = "ffmpeg.exe" },
  @{ Name = "ffprobe"; Version = "N-128728-g49a3b1f259"; Kind = "zip-dir"
     Pin = "acb67f124da510d346c0f2afe19face6f316f0d8959b595ad5050b1b86bfbcbc"
     Url  = "https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/download/autobuild-2026-11-10-12-39/ffmpeg-N-128728-g49a3b1f259-win64-lgpl-shared.zip"
     Dest = "bin/ffprobe.exe"; ExeName = "ffprobe.exe" },
  @{ Name = "piper"; Version = "2023.11.14-2"; Kind = "zip-dir"
     Pin = "f3c58906402b24f3a96d92145f58acba6d86c9b5db896d207f78dc80811efcea"
     Url  = "https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_windows_amd64.zip"
     Dest = "bin/piper.exe"; ExeName = "piper.exe" },
  @{ Name = "qjs"; Version = "v0.18.0"; Kind = "file"
     Pin = "ff03398d74ea6e6bd3a8e58058f7003225abcac229d7d57775b7aaae812a49c7"
     Url  = "https://github.com/quickjs-ng/quickjs/releases/download/v0.18.0/qjs-windows-x86_64.exe"
     Dest = "bin/qjs.exe" },
  @{ Name = "voice-onnx"; Version = "ngochuyennew"; Kind = "file"
     Pin = "88bc21477831dd99759cff164f0ab270e258faf435d01741f0211ff8620255e2"
     Url  = "https://huggingface.co/buckets/renzynx/nghitts-copy-bucket/resolve/piper-tts/ngochuyennew.onnx?download=true"
     Dest = "models/ngochuyennew.onnx" },
  @{ Name = "voice-config"; Version = "nghitts-shared"; Kind = "file"
     Pin = "6ce7b1bac4cf40410cd3c75fb100475306877850f5697f3d4daeeb3225410271"
     Url  = "https://huggingface.co/buckets/renzynx/nghitts-copy-bucket/resolve/piper-tts/config.json?download=true"
     Dest = "models/config.json" }
)

$ArchiveCache = @{}

function Get-Archive([hashtable]$a) {
  if ($ArchiveCache.ContainsKey($a.Pin)) { return $ArchiveCache[$a.Pin] }
  $leaf = Split-Path -Leaf ($a.Url -split '[?#]')[0]
  $archivePath = Join-Path $CacheDir $leaf
  $extDir = Join-Path $ExtRoot ([IO.Path]::GetFileNameWithoutExtension($leaf))

  if (Test-Path -LiteralPath $archivePath) {
    if ((Get-FileSha256 $archivePath) -eq $a.Pin) {
      Write-Host "  cache: ${leaf} (SHA-256 matches pin, no download)"
    } else {
      Write-Host "  cache: ${leaf} present but SHA-256 mismatch - re-downloading"
      if ($PSCmdlet.ShouldProcess($leaf, "Delete stale cache")) { Remove-Item -LiteralPath $archivePath -Force }
    }
  }
  if (-not (Test-Path -LiteralPath $archivePath)) {
    if (-not $PSCmdlet.ShouldProcess($leaf, "Download from $($a.Url)")) {
      Write-Host "[DRY-RUN] would download ${leaf}"
      return $null
    }
    if (-not (Test-Path -LiteralPath $CacheDir)) { New-Item -ItemType Directory -Force -Path $CacheDir > $null }
    $attempt = 0
    while ($true) {
      try {
        $attempt++
        Invoke-WebRequest -Uri $a.Url -OutFile $archivePath -UseBasicParsing
        break
      } catch {
        if ($attempt -ge 3) { throw "download failed ($($a.Name)): $($_.Exception.Message)" }
        Start-Sleep -Seconds (2 * $attempt)
      }
    }
    $actual = Get-FileSha256 $archivePath
    if ($actual -ne $a.Pin) {
      throw "SHA-256 mismatch for ${leaf}:`n  expected $($a.Pin)`n  actual   $actual`nRefusing to stage. Delete the cache and retry; if upstream changed, bump the pin in both fetch scripts."
    }
    Write-Host "  verified: ${leaf} (SHA-256 matches pin)"
  }
  if ($a.Kind -ne "file" -and -not (Test-Path -LiteralPath $extDir)) {
    if ($PSCmdlet.ShouldProcess($leaf, "Extract to $extDir")) {
      if (-not (Test-Path -LiteralPath $extDir)) { New-Item -ItemType Directory -Force -Path $extDir > $null }
      Expand-Archive -LiteralPath $archivePath -DestinationPath $extDir -Force
    }
  }
  $ArchiveCache[$a.Pin] = @($archivePath, $extDir)
  return $ArchiveCache[$a.Pin]
}

function Stage-FileArtifact([hashtable]$a) {
  $archive = Get-Archive $a
  $destPath = Join-Path $DistDir $a.Dest
  if ($null -eq $archive) { return $destPath }
  $parent = Split-Path -Parent $destPath
  if ($PSCmdlet.ShouldProcess($destPath, "Stage from download")) {
    if (-not (Test-Path -LiteralPath $parent)) { New-Item -ItemType Directory -Force -Path $parent > $null }
    Copy-Item -LiteralPath $archive[0] -Destination $destPath -Force
  } else {
    Write-Host "[DRY-RUN] would stage ${destPath}"
  }
  return $destPath
}

function Stage-DirArtifact([hashtable]$a) {
  $archive = Get-Archive $a
  $destPath = Join-Path $DistDir $a.Dest
  if ($null -eq $archive) { return $destPath }
  $extDir = $archive[1]
  if (-not (Test-Path -LiteralPath $extDir)) {
    Write-Host "[DRY-RUN] would extract + stage from $($a.Url)"
    return $destPath
  }
  $exe = Get-ChildItem -LiteralPath $extDir -Filter $a.ExeName -Recurse -File | Select-Object -First 1
  if (-not $exe) { throw "staged archive for $($a.Name) does not contain $($a.ExeName)" }
  $exeDir = Split-Path -Parent $exe.FullName
  $destDir = Join-Path $DistDir (Split-Path -Parent $a.Dest)
  if ($PSCmdlet.ShouldProcess("$($a.ExeName) dir", "Stage dir -> $destDir")) {
    if (-not (Test-Path -LiteralPath $destDir)) { New-Item -ItemType Directory -Force -Path $destDir > $null }
    Copy-Item (Join-Path $exeDir "*") $destDir -Recurse -Force
  } else {
    Write-Host "[DRY-RUN] would stage dir ${exeDir} -> ${destDir}"
  }
  return $destPath
}

function Write-AtomicText([string]$Path, [string]$Content) {
  $tmp = "$Path.tmp.$PID"
  if (-not $PSCmdlet.ShouldProcess($Path, "Write")) { Write-Host "[DRY-RUN] would write ${Path}"; return }
  $parent = Split-Path -Parent $Path
  if (-not (Test-Path -LiteralPath $parent)) { New-Item -ItemType Directory -Force -Path $parent > $null }
  [IO.File]::WriteAllText($tmp, $Content)
  Move-Item -LiteralPath $tmp -Destination $Path -Force
}

if ($IsDryRun) {
  Write-Host "[DRY-RUN] would create ${BinDir}, ${ModelsDir}, ${StateDir}"
} else {
  foreach ($d in @($BinDir, $ModelsDir, $StateDir)) {
    if (-not (Test-Path -LiteralPath $d)) { New-Item -ItemType Directory -Force -Path $d > $null }
  }
}

$manifestList = @()
foreach ($a in $Artifacts) {
  Write-Host "== $($a.Name) ($($a.Version))"
  $stagedPath = Join-Path $DistDir $a.Dest
  $markerPath = Join-Path $StateDir (([IO.Path]::GetFileName($a.Dest) -replace '[^A-Za-z0-9._-]', '_') + ".sha256")
  $upToDate = $false
  $stagedHash = $null
  if (Test-Path -LiteralPath $stagedPath) {
    $stagedHash = Get-FileSha256 $stagedPath
    if ($stagedHash -eq $a.Pin) { $upToDate = $true }
    if ((-not $upToDate) -and (Test-Path -LiteralPath $markerPath)) {
      if ([IO.File]::ReadAllText($markerPath).Trim().ToLowerInvariant() -eq $stagedHash) { $upToDate = $true }
    }
  }
  if ($upToDate) {
    Write-Host "  up to date (skip, no download)"
    $manifestList += New-JsonEntry $a $stagedHash
    continue
  }
  if ($IsDryRun) {
    Write-Host "[DRY-RUN] would download $($a.Url) -> ${stagedPath}"
    continue
  }
  if ($a.Kind -eq "file") { $stagedPath = Stage-FileArtifact $a }
  else                   { $stagedPath = Stage-DirArtifact  $a }
  $stagedHash = Get-FileSha256 $stagedPath
  Write-AtomicText $markerPath ($stagedHash + "`n")
  $manifestList += New-JsonEntry $a $stagedHash
}

$manifestJson = ($manifestList | ConvertTo-Json -Depth 4) + "`n"
Write-AtomicText (Join-Path $DistDir "versions.json") $manifestJson
if ($IsDryRun) {
  Write-Host "Done (dry run - nothing written)."
} else {
  Write-Host "versions.json written: $(($manifestList).Count) artifact(s) in ${DistDir}"
  Write-Host "Done. Stage dist/bin, models + versions.json into the installer."
}

# tsr installer for Windows (PowerShell).
#
#   irm https://raw.githubusercontent.com/Open-Tech-Foundation/tsr/main/install.ps1 | iex
#
# Downloads the latest released `tsr` binary for your platform, verifies its
# SHA-256 checksum when the release ships one, and installs it to
# $HOME\.tsr\bin. Override the version with $env:TSR_VERSION = 'v0.1.0' and
# the install dir with $env:TSR_INSTALL = 'C:\custom\path'.

$ErrorActionPreference = 'Stop'

$Repo = 'Open-Tech-Foundation/tsr'
$InstallDir = if ($env:TSR_INSTALL) { $env:TSR_INSTALL } else { Join-Path $HOME '.tsr' }
$BinDir = Join-Path $InstallDir 'bin'

# --- detect platform --------------------------------------------------------
$arch = switch ($env:PROCESSOR_ARCHITECTURE) {
  'AMD64' { 'x86-64' }
  'ARM64' { 'arm64' }
  default { throw "unsupported architecture: $($env:PROCESSOR_ARCHITECTURE)" }
}
$target = "win32-$arch"

# --- resolve version --------------------------------------------------------
$version = $env:TSR_VERSION
if (-not $version) {
  $rel = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest"
  $version = $rel.tag_name
}
if (-not $version) { throw 'could not determine the latest release (set $env:TSR_VERSION)' }
$name = "tsr-$target"
$url = "https://github.com/$Repo/releases/download/$version/$name.zip"

Write-Host "Installing tsr $version ($target)" -ForegroundColor Cyan
Write-Host "  from $url" -ForegroundColor DarkGray

# --- download + verify ------------------------------------------------------
$tmp = Join-Path $env:TEMP ("tsr-" + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
  $zip = Join-Path $tmp "$name.zip"
  try {
    Invoke-WebRequest -Uri $url -OutFile $zip
  } catch {
    throw "download failed - is there a release asset for $target?"
  }

  # Checksums, when present, live in one `checksums.txt` per release
  # (`<hash>  <archive>` lines); pull out the line for our archive and verify
  # it. A release without a checksums.txt is not fatal - verification is skipped.
  $sumFile = Join-Path $tmp 'checksums.txt'
  $sumsUrl = "https://github.com/$Repo/releases/download/$version/checksums.txt"
  $line = $null
  try {
    Invoke-WebRequest -Uri $sumsUrl -OutFile $sumFile
    $line = Get-Content $sumFile | Where-Object { $_ -match "  $([regex]::Escape($name)).zip$" } | Select-Object -First 1
  } catch {}
  if ($line) {
    $expected = (($line -split '\s+')[0]).ToLower()
    $actual = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
    if ($expected -ne $actual) { throw 'checksum verification failed' }
    Write-Host '  checksum verified' -ForegroundColor DarkGray
  } else {
    Write-Host '  no checksums.txt for this release - skipping verification' -ForegroundColor DarkGray
  }

  # --- install --------------------------------------------------------------
  Expand-Archive -Path $zip -DestinationPath $tmp -Force
  New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
  Copy-Item (Join-Path $tmp 'tsr.exe') (Join-Path $BinDir 'tsr.exe') -Force

  Write-Host ''
  Write-Host "tsr was installed to $BinDir\tsr.exe"

  # Add to the user PATH if it isn't already there.
  $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  if (($userPath -split ';') -notcontains $BinDir) {
    [Environment]::SetEnvironmentVariable('Path', "$BinDir;$userPath", 'User')
    Write-Host "Added $BinDir to your user PATH - restart your shell to pick it up."
  }
  Write-Host "Run 'tsr --version' to verify." -ForegroundColor DarkGray
} finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

$ErrorActionPreference = "Stop"

# Compile once so every generated test executable can receive the same Common
# Controls v6 activation context that Tauri embeds in the shipped application.
cargo test --manifest-path src-tauri/Cargo.toml --all-targets --no-run
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$kitsRoot = "${env:ProgramFiles(x86)}/Windows Kits/10/bin"
$manifestTool = Get-ChildItem $kitsRoot -Filter mt.exe -Recurse |
  Where-Object { $_.FullName -match '\\x64\\mt\.exe$' } |
  Sort-Object FullName -Descending |
  Select-Object -First 1
if (-not $manifestTool) { throw "mt.exe was not found on the Windows runner." }

$testExecutables = Get-ChildItem "src-tauri/target/debug/deps/*.exe"
if (-not $testExecutables) { throw "Windows Rust test executables were not produced." }

$manifest = (Resolve-Path "scripts/windows-test.manifest").Path
foreach ($executable in $testExecutables) {
  & $manifestTool.FullName -nologo -manifest $manifest "-outputresource:$($executable.FullName);#1"
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

cargo test --manifest-path src-tauri/Cargo.toml --all-targets
exit $LASTEXITCODE

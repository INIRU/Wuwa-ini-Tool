$ErrorActionPreference = "Stop"

# Compile once and retain Cargo's exact test-executable list. Running the files
# directly after manifest injection avoids a second Cargo link pass replacing
# the patched PE resources.
$messages = cargo test --manifest-path src-tauri/Cargo.toml --all-targets --no-run --message-format=json
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$testExecutables = $messages |
  ForEach-Object {
    try { $_ | ConvertFrom-Json } catch { $null }
  } |
  Where-Object { $_.reason -eq "compiler-artifact" -and $_.profile.test -and $_.executable } |
  ForEach-Object { $_.executable } |
  Sort-Object -Unique
if (-not $testExecutables) { throw "Cargo did not report any Windows Rust test executables." }

$kitsRoot = "${env:ProgramFiles(x86)}/Windows Kits/10/bin"
$manifestTool = Get-ChildItem $kitsRoot -Filter mt.exe -Recurse |
  Where-Object { $_.FullName -match '\\x64\\mt\.exe$' } |
  Sort-Object FullName -Descending |
  Select-Object -First 1
if (-not $manifestTool) { throw "mt.exe was not found on the Windows runner." }

$manifest = (Resolve-Path "scripts/windows-test.manifest").Path
foreach ($executable in $testExecutables) {
  & $manifestTool.FullName -nologo -manifest $manifest "-outputresource:$executable;#1"
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

foreach ($executable in $testExecutables) {
  Write-Host "Running Windows Rust test executable: $executable"
  & $executable
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

$ErrorActionPreference = "Stop"

$binDir = "$env:USERPROFILE\.local\bin"
$exe = "$binDir\claude-overlay.exe"

Write-Host "Copying binary to $binDir..."
New-Item -ItemType Directory -Force -Path $binDir | Out-Null
Copy-Item .\target\release\claude-overlay.exe $exe -Force

$p = [Environment]::GetEnvironmentVariable("PATH", "User")
if ($p -notlike "*$binDir*") {
  Write-Host "Adding $binDir to user PATH..."
  [Environment]::SetEnvironmentVariable("PATH", "$p;$binDir", "User")
}

Write-Host "Installing auto-start..."
& $exe --install-autostart

$vsix = Get-ChildItem vscode-ext\*.vsix | Select-Object -First 1
if ($vsix) {
  Write-Host "Installing VS Code extension $($vsix.Name)..."
  code --install-extension $vsix.FullName
}

Write-Host "Starting daemon..."
Start-Process $exe -ArgumentList "--daemon" -WindowStyle Hidden

Write-Host "Done. Run 'claude-overlay.exe --status' to verify."

# claude-overlay Installation

## Prerequisites

- Windows 11 (WebView2 runtime pre-installed)
- WSL2 Ubuntu (ou autre distro) avec `jq` (`sudo apt install -y jq`)
- VS Code sur Windows (si tu utilises VS Code Remote-WSL)
- `claude-overlay.exe` (release binary) — produit par `cargo tauri build` ou téléchargé depuis ce repo

## Install — one-time

### Windows side (PowerShell)

1. Copie le binaire :
   ```powershell
   mkdir "$env:USERPROFILE\.local\bin" -Force
   Copy-Item .\target\release\claude-overlay.exe "$env:USERPROFILE\.local\bin\"
   ```

2. Ajoute au PATH utilisateur (persistent) :
   ```powershell
   $p = [Environment]::GetEnvironmentVariable("PATH", "User")
   [Environment]::SetEnvironmentVariable("PATH", "$p;$env:USERPROFILE\.local\bin", "User")
   ```
   Redémarre ton shell pour prise d'effet.

3. Installe l'auto-start Windows (HKCU, no admin) :
   ```powershell
   & "$env:USERPROFILE\.local\bin\claude-overlay.exe" --install-autostart
   ```

4. Installe l'extension VS Code :
   ```powershell
   code --install-extension vscode-ext\claude-overlay-focus-0.1.0.vsix
   ```

5. Démarre le daemon maintenant (il redémarrera au prochain login automatiquement) :
   ```powershell
   Start-Process "$env:USERPROFILE\.local\bin\claude-overlay.exe" -ArgumentList "--daemon" -WindowStyle Hidden
   ```

### WSL side (bash)

1. Copie le hook :
   ```bash
   mkdir -p ~/.claude/hooks
   cp hooks/claude-overlay-notify.sh ~/.claude/hooks/
   chmod +x ~/.claude/hooks/claude-overlay-notify.sh
   ```

2. Patche `~/.claude/settings.json` (section `hooks`) pour pointer le hook sur events `Notification` et `Stop`. Voir exemple dans le spec §8.

## Verify

- `claude-overlay.exe --status` → `daemon is running`
- VS Code Command Palette → "Claude Overlay: Show connection status" → `connected`
- Dans une session Claude Code WSL, fire un prompt qui déclenche une notif → overlay apparaît top-center.

## Uninstall

```powershell
claude-overlay.exe --uninstall-autostart
Stop-Process -Name claude-overlay -Force
Remove-Item "$env:USERPROFILE\.local\bin\claude-overlay.exe"
code --uninstall-extension adrie-local.claude-overlay-focus
```

```bash
rm ~/.claude/hooks/claude-overlay-notify.sh
# revert settings.json manually
```

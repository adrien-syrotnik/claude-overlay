# Claude Overlay — Finir l'installation

La partie Windows est installée et le daemon tourne déjà. Il reste à configurer le côté WSL (extension VS Code + hook bash). Le plus simple: laisser Claude Code s'en occuper.

## 1. Lance Claude Code

Ouvre VS Code en mode **Remote-WSL**, puis lance Claude Code dans un terminal intégré.

## 2. Copie-colle ce prompt dans Claude Code

```
Tu es invoqué pour finaliser l'installation de claude-overlay sur cette machine.
Le daemon Windows est déjà installé et lancé. Il te reste à configurer le côté WSL.

Tâches à exécuter dans l'ordre — confirme chaque étape avant de passer à la suivante:

1. Installe l'extension VS Code:
   code --install-extension '/mnt/c/Program Files/claude-overlay/resources/claude-overlay-focus.vsix'

2. Copie le script hook:
   mkdir -p ~/.claude/hooks
   cp '/mnt/c/Program Files/claude-overlay/resources/claude-overlay-notify.sh' ~/.claude/hooks/
   chmod +x ~/.claude/hooks/claude-overlay-notify.sh

3. Enregistre le hook dans ~/.claude/settings.json sur les events Notification, Stop,
   et PreToolUse (matcher AskUserQuestion). Fusionne avec les hooks existants sans
   rien écraser. Pour Notification et Stop, ajoute:
     { "matcher": "", "hooks": [
         { "type": "command", "command": "bash ~/.claude/hooks/claude-overlay-notify.sh" }
       ] }
   Pour PreToolUse, utilise matcher "AskUserQuestion" et timeout 600.

4. Vérifie que le daemon répond:
   claude-overlay.exe --status
   doit imprimer "daemon is running".

5. Smoke test — envoie une notif fictive:
   printf '%s' '{"event":"Stop","cwd":"/tmp","message":"setup test","source_type":"vscode","source_basename":"setup","wt_session":"","vscode_ipc_hook":"","vscode_pid":"","shell_pid":0,"timestamp_ms":0,"notification_type":""}' | claude-overlay.exe --stdin
   Tu dois voir un overlay apparaître brièvement en haut de l'écran.

6. Recharge la fenêtre VS Code (Ctrl+Shift+P → "Developer: Reload Window") pour que
   l'extension claude-overlay-focus s'active.

Diagnostique avant de continuer si une commande échoue. Pour debug, le daemon
logge dans %USERPROFILE%\claude-overlay.log.
```

## 3. Ferme cette fenêtre quand Claude a fini

C'est tout. Tes prochaines sessions Claude Code feront apparaître l'overlay en haut de l'écran.

---

## Debug

- Log du daemon: `%USERPROFILE%\claude-overlay.log`
- Vérifier que les ports sont libres: `Test-NetConnection 127.0.0.1 -Port 47842`
- Tuer le daemon: `Stop-Process -Name claude-overlay -Force`
- Relancer manuellement: `& "C:\Program Files\claude-overlay\claude-overlay.exe" --daemon`

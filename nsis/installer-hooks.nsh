; Claude Overlay NSIS installer hooks.
; Wires HKCU Run auto-start at install, removes it at uninstall, and launches
; the daemon immediately post-install so the user doesn't have to log out/in.

!macro NSIS_HOOK_POSTINSTALL
  DetailPrint "Registering claude-overlay for auto-start..."
  ExecWait '"$INSTDIR\claude-overlay.exe" --install-autostart' $0
  DetailPrint "  --install-autostart exit code: $0"
  ; Detached launch so the installer doesn't wait on the daemon's main loop.
  Exec '"$INSTDIR\claude-overlay.exe" --daemon'
  ; Open the setup guide in the default markdown handler (typically VS Code on
  ; dev machines; falls back to whatever the user has). Gives them the Claude
  ; Code prompt they need to copy to finish the WSL-side setup.
  ExecShell "open" "$INSTDIR\resources\CLAUDE_SETUP.md"
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  DetailPrint "Removing claude-overlay auto-start..."
  ExecWait '"$INSTDIR\claude-overlay.exe" --uninstall-autostart' $0
  DetailPrint "  --uninstall-autostart exit code: $0"
!macroend

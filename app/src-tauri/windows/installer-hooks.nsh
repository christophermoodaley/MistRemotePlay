; Mist Remote Play installer hooks (Tauri NSIS).
; Inbound firewall rules let other PCs reach the built-in signaling server
; and the WebRTC media ports without Windows asking on first use. Every
; session is still authenticated with paired keys and needs host approval.

!macro NSIS_HOOK_POSTINSTALL
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="Mist Remote Play"'
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="Mist Remote Play (streaming client)"'
  nsExec::ExecToLog 'netsh advfirewall firewall add rule name="Mist Remote Play" dir=in action=allow program="$INSTDIR\${MAINBINARYNAME}.exe" enable=yes profile=any'
  nsExec::ExecToLog 'netsh advfirewall firewall add rule name="Mist Remote Play (streaming client)" dir=in action=allow program="$INSTDIR\mist.exe" enable=yes profile=any'
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="Mist Remote Play"'
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="Mist Remote Play (streaming client)"'
!macroend

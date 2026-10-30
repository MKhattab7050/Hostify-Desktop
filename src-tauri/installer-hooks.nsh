!include "nsDialogs.nsh"
!include "LogicLib.nsh"

Var HostifyDeleteData
Var HostifyDeleteCheckbox

; A separate, unchecked opt-in page; cancelling never deletes data.
UninstPage custom un.HostifyDataPage un.HostifyDataLeave
Function un.HostifyDataPage
  StrCpy $HostifyDeleteData ${BST_UNCHECKED}
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    Abort
  ${EndIf}
  ${NSD_CreateLabel} 0 0 100% 35u "Hostify keeps your websites by default. Hosting stops when the engine is uninstalled."
  Pop $0
  ${NSD_CreateCheckbox} 0 45u 100% 20u "Delete my website data (cannot be undone)"
  Pop $HostifyDeleteCheckbox
  ${NSD_SetState} $HostifyDeleteCheckbox ${BST_UNCHECKED}
  nsDialogs::Show
FunctionEnd
Function un.HostifyDataLeave
  ${NSD_GetState} $HostifyDeleteCheckbox $HostifyDeleteData
FunctionEnd

!macro NSIS_HOOK_PREINSTALL
  ; During upgrades release the existing engine binary before replacing it.
  ${If} ${FileExists} "$INSTDIR\stop-engine.ps1"
    nsExec::ExecToStack 'powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\stop-engine.ps1" -EnginePath "$INSTDIR\hostify-engine.exe"'
    Pop $0
    Pop $1
    ${If} $0 != 0
      MessageBox MB_OK|MB_ICONSTOP "Hostify could not stop the old engine for this upgrade. Close it and retry."
      Abort
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  nsExec::ExecToStack '"$INSTDIR\hostify-engine.exe" install-task'
  Pop $0
  Pop $1
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONEXCLAMATION "Hostify was installed, but automatic startup could not be configured. Open Hostify settings to retry."
  ${EndIf}
  Exec '"$INSTDIR\hostify-engine.exe"'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Stop only the engine at this installation path; its Job Object closes children.
  nsExec::ExecToStack 'powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\stop-engine.ps1" -EnginePath "$INSTDIR\hostify-engine.exe"'
  Pop $0
  Pop $1
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONSTOP "Hostify could not stop the engine. Close the running engine and retry uninstall. Website data has been kept."
    Abort
  ${EndIf}
  nsExec::ExecToStack 'powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\remove-task.ps1" -EnginePath "$INSTDIR\hostify-engine.exe"'
  Pop $0
  Pop $1
  ${If} $0 != 0
    MessageBox MB_OK|MB_ICONSTOP "Hostify could not remove its startup task. Retry uninstall after closing Hostify. Website data has been kept."
    Abort
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $HostifyDeleteData == ${BST_CHECKED}
    SetShellVarContext current
    RMDir /r "$LOCALAPPDATA\Hostify\sites"
  ${EndIf}
!macroend

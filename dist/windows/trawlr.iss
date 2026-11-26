; The Windows installer, built with Inno Setup 6 by dist\windows\build.ps1:
;   iscc /DAppVersion=0.1.0 /DStage=<folder with the programs> trawlr.iss
;
; It installs the programs, registers and starts the trawlrd service (which
; indexes the NTFS drives), and adds Trawlr to the Start menu.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef Stage
  #define Stage "..\out\windows-stage"
#endif

[Setup]
AppId={{1E6D7B05-56DD-4297-89DC-ADDD67670AAD}
AppName=Trawlr
AppVersion={#AppVersion}
AppVerName=Trawlr {#AppVersion}
AppPublisher=Sam Gilbert
AppPublisherURL=https://trawlr.io
AppSupportURL=https://github.com/SamuelCGilbert/trawlr/issues
AppUpdatesURL=https://github.com/SamuelCGilbert/trawlr/releases
DefaultDirName={autopf}\Trawlr
DisableProgramGroupPage=yes
OutputDir=..\out
OutputBaseFilename=trawlr-{#AppVersion}-windows-x64-setup
SetupIconFile=..\..\assets\app-icon.ico
UninstallDisplayIcon={app}\trawlr-gui.exe
UninstallDisplayName=Trawlr
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; The service reads the drives directly, which takes an administrator.
PrivilegesRequired=admin
MinVersion=10.0
ChangesEnvironment=yes
CloseApplications=yes
RestartApplications=no

[Tasks]
Name: autostart; Description: "Start Trawlr when I sign in (in the tray, so the global hotkeys work)"
Name: addtopath; Description: "Add the trawlr command to PATH"

[Files]
Source: "{#Stage}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "config.toml"; DestDir: "{commonappdata}\Trawlr"; Flags: onlyifdoesntexist

[Icons]
Name: "{autoprograms}\Trawlr"; Filename: "{app}\trawlr-gui.exe"; Comment: "Find any file by name"

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Trawlr"; ValueData: """{app}\trawlr-gui.exe"" --background"; Tasks: autostart; Flags: uninsdeletevalue
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Tasks: addtopath; Check: NotInPath(ExpandConstant('{app}'))

[Run]
Filename: "{app}\trawlrd.exe"; Parameters: "--install-service"; Flags: runhidden waituntilterminated; StatusMsg: "Starting the indexing service..."
; Back in the tray if setup had to close it.
Filename: "{app}\trawlr-gui.exe"; Parameters: "--background"; Flags: nowait runasoriginaluser; Check: GuiWasRunning
Filename: "{app}\trawlr-gui.exe"; Description: "Open Trawlr"; Flags: postinstall nowait skipifsilent runasoriginaluser

[UninstallRun]
Filename: "{app}\trawlrd.exe"; Parameters: "--uninstall-service"; Flags: runhidden waituntilterminated; RunOnceId: "RemoveService"
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM trawlr-gui.exe"; Flags: runhidden waituntilterminated; RunOnceId: "CloseWindows"

[UninstallDelete]
; The index, the log and the configuration.
Type: filesandordirs; Name: "{commonappdata}\Trawlr"

[Code]
const
  EnvKey = 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment';

var
  ClosedGui: Boolean;

function GuiWasRunning: Boolean;
begin
  Result := ClosedGui;
end;

function NotInPath(Dir: string): Boolean;
var
  Path: string;
begin
  if not RegQueryStringValue(HKLM, EnvKey, 'Path', Path) then
    Result := True
  else
    Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Path) + ';') = 0;
end;

// An upgrade replaces trawlrd.exe, which the running service holds open:
// stop and remove the service first; installing registers it again. The
// same goes for the programs' DLLs, which running windows hold open.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
  Old: string;
begin
  Old := ExpandConstant('{app}\trawlrd.exe');
  if FileExists(Old) then
    Exec(Old, '--uninstall-service', '', SW_HIDE, ewWaitUntilTerminated, Code);
  // A Trawlr window holds its files open too, and it may be another user's,
  // which closing applications the usual way cannot reach (it only sees the
  // session setup runs in). taskkill exits with 0 when it ended something.
  ClosedGui := Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM trawlr-gui.exe', '', SW_HIDE, ewWaitUntilTerminated, Code) and (Code = 0);
  Result := '';
end;

// Take the folder back out of PATH.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Path, Dir: string;
  P: Integer;
begin
  if CurUninstallStep <> usPostUninstall then
    exit;
  if not RegQueryStringValue(HKLM, EnvKey, 'Path', Path) then
    exit;
  Dir := ';' + ExpandConstant('{app}');
  // Followed by ';' (or the end), so C:\...\Trawlr2 is not taken for it.
  P := Pos(Uppercase(Dir) + ';', Uppercase(Path) + ';');
  if P > 0 then
  begin
    Delete(Path, P, Length(Dir));
    RegWriteExpandStringValue(HKLM, EnvKey, 'Path', Path);
  end;
end;

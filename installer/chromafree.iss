#define AppVersion GetEnv("CHROMAFREE_VERSION")
#define PackageDir GetEnv("CHROMAFREE_PACKAGE_DIR")
#define OutputDir GetEnv("CHROMAFREE_INSTALLER_DIR")

[Setup]
AppId={{B1F6639B-9B28-461F-B14A-705F071D3D08}
AppName=ChromaFree
AppVersion={#AppVersion}
AppVerName=ChromaFree {#AppVersion}
AppPublisher=ChromaFree
DefaultDirName={autopf}\ChromaFree
DefaultGroupName=ChromaFree
DisableProgramGroupPage=yes
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.22000
AppMutex=chromafree-app-instance
CloseApplications=yes
RestartApplications=no
LicenseFile={#PackageDir}\LICENSE
SetupIconFile=..\crates\chromafree-app\assets\chromafree.ico
UninstallDisplayIcon={app}\chromafree.exe
UninstallDisplayName=ChromaFree
OutputDir={#OutputDir}
OutputBaseFilename=ChromaFree-Setup-{#AppVersion}
Compression=lzma2/ultra64
SolidCompression=yes
LZMANumBlockThreads=4
WizardStyle=modern

[Languages]
Name: "polish"; MessagesFile: "compiler:Languages\Polish.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
polish.AutoStart=Uruchamiaj ChromaFree w tle po zalogowaniu
english.AutoStart=Start ChromaFree in the background when I sign in
polish.LaunchApp=Uruchom ChromaFree
english.LaunchApp=Launch ChromaFree

[Tasks]
Name: "autostart"; Description: "{cm:AutoStart}"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#PackageDir}\chromafree.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PackageDir}\DirectML.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PackageDir}\vcam-source.dll"; DestDir: "{app}"; Flags: ignoreversion regserver restartreplace uninsrestartdelete
Source: "{#PackageDir}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PackageDir}\THIRD_PARTY.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PackageDir}\VCAM_THIRD_PARTY.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PackageDir}\models\*"; DestDir: "{app}\models"; Flags: ignoreversion

[InstallDelete]
Type: files; Name: "{app}\models\*.onnx"

[Icons]
Name: "{autoprograms}\ChromaFree"; Filename: "{app}\chromafree.exe"
Name: "{autodesktop}\ChromaFree"; Filename: "{app}\chromafree.exe"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "ChromaFree"; ValueData: """{app}\chromafree.exe"" --minimized"; Tasks: autostart; Flags: uninsdeletevalue

[Run]
Filename: "{app}\chromafree.exe"; Description: "{cm:LaunchApp}"; Flags: nowait postinstall skipifsilent runasoriginaluser

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/IM chromafree.exe /F"; Flags: runhidden; RunOnceId: "StopChromaFree"

[UninstallDelete]
Type: filesandordirs; Name: "{app}\models"

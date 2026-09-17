#define AppVersion GetEnv("CHROMAFREE_VERSION")
#define PackageDir GetEnv("CHROMAFREE_PACKAGE_DIR")
#define OutputDir GetEnv("CHROMAFREE_INSTALLER_DIR")
#define AppAuthor GetEnv("CHROMAFREE_AUTHOR")
#define AppUrl GetEnv("CHROMAFREE_URL")

[Setup]
AppId={{B1F6639B-9B28-461F-B14A-705F071D3D08}
AppName=ChromaFree
AppVersion={#AppVersion}
AppVerName=ChromaFree {#AppVersion}
AppPublisher={#AppAuthor}
AppPublisherURL={#AppUrl}
AppSupportURL={#AppUrl}/issues
AppUpdatesURL={#AppUrl}/releases
AppCopyright=Copyright (C) 2026 {#AppAuthor}
AppComments=Virtual camera that removes, replaces or blurs the webcam background on the GPU
VersionInfoVersion={#AppVersion}
VersionInfoCompany={#AppAuthor}
VersionInfoDescription=ChromaFree Setup
VersionInfoProductName=ChromaFree
VersionInfoProductVersion={#AppVersion}
VersionInfoCopyright=Copyright (C) 2026 {#AppAuthor}
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
Compression=lzma2/max
LZMAUseSeparateProcess=yes
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
polish.InstallingCamera=Dodawanie kamery ChromaFree do systemu...
english.InstallingCamera=Adding the ChromaFree camera to the system...

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
Source: "{#PackageDir}\offline.png"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PackageDir}\models\*"; DestDir: "{app}\models"; Flags: ignoreversion

[InstallDelete]
Type: files; Name: "{app}\models\*.onnx"

[Icons]
Name: "{autoprograms}\ChromaFree"; Filename: "{app}\chromafree.exe"
Name: "{autodesktop}\ChromaFree"; Filename: "{app}\chromafree.exe"; Tasks: desktopicon

[Registry]
Root: HKLM; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "ChromaFree"; ValueData: """{app}\chromafree.exe"" --minimized"; Tasks: autostart; Flags: uninsdeletevalue

[Run]
Filename: "{app}\chromafree.exe"; Parameters: "--install-camera"; StatusMsg: "{cm:InstallingCamera}"; Flags: runhidden waituntilterminated
Filename: "{app}\chromafree.exe"; Description: "{cm:LaunchApp}"; Flags: nowait postinstall skipifsilent runasoriginaluser

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/IM chromafree.exe /F"; Flags: runhidden waituntilterminated; RunOnceId: "StopChromaFree"
Filename: "{app}\chromafree.exe"; Parameters: "--uninstall-camera"; Flags: runhidden waituntilterminated; RunOnceId: "RemoveCamera"

[UninstallDelete]
Type: filesandordirs; Name: "{app}\models"

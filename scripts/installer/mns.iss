; Inno Setup 脚本：由 scripts/make-installer.ps1 调用，不要单独双击编译。
;
; 脚本通过命令行 /D 传入以下宏（见 make-installer.ps1）：
;   AppVersion   Cargo.toml 里的版本号
;   SourceDir    已放好 mns-gui.exe / mns.exe / LICENSE 的暂存目录
;   OutputDir    安装包输出目录（target\dist）
;   IconFile     assets\app.ico
;   LicenseFile  LICENSE
;   ArchName     x64（用于产物文件名）
;
; 每个宏都有默认值，方便在 Inno Setup IDE 里直接打开看结构；默认值对应
; 在仓库根目录执行脚本后的产物位置。

#ifndef AppVersion
  #define AppVersion "0.8.0"
#endif

#ifndef SourceDir
  #define SourceDir "..\..\target\dist\stage"
#endif

#ifndef OutputDir
  #define OutputDir "..\..\target\dist"
#endif

#ifndef IconFile
  #define IconFile "..\..\assets\app.ico"
#endif

#ifndef LicenseFile
  #define LicenseFile "..\..\LICENSE"
#endif

#ifndef ArchName
  #define ArchName "x64"
#endif

#define AppName "MNS"
#define AppDisplayName "MNS 投资助手"
#define AppPublisher "Sopaco"
#define AppExeName "mns-gui.exe"
#define CliExeName "mns.exe"
#define HelpExeName "mns-help.exe"
#define ButlerExeName "mns-butler.exe"
#define ButlerdExeName "mns-butlerd.exe"
#define AppUrl "https://github.com/sopaco/money-never-sleep"

[Setup]
; AppId 是升级/卸载的唯一标识，任何情况下都不要改。
AppId={{7B2C3D1E-5A64-4F8B-9C2D-3E1F5A7B9C0D}
AppName={#AppDisplayName}
AppVersion={#AppVersion}
AppVerName={#AppDisplayName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppUrl}
AppSupportURL={#AppUrl}
AppUpdatesURL={#AppUrl}
; 按用户安装：装到 %LOCALAPPDATA%\Programs\MNS，不弹 UAC，也不需要管理员。
DefaultDirName={autopf}\MNS
DefaultGroupName={#AppDisplayName}
DisableProgramGroupPage=yes
AllowNoIcons=yes
PrivilegesRequired=lowest
OutputDir={#OutputDir}
OutputBaseFilename=MNS-{#AppVersion}-win-{#ArchName}-setup
SetupIconFile={#IconFile}
; 图标由 mns-gui.exe 内嵌的 ID-1 资源提供（build.rs 按 bin 编入 app.ico），
; 不再把一个单独的 app.ico 释放到安装目录。
UninstallDisplayIcon={app}\{#AppExeName}
UninstallDisplayName={#AppDisplayName}
LicenseFile={#LicenseFile}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
MinVersion=10.0
; 按用户安装，没有 Program Files / HKLM 重定向问题，因此不需要 64 位安装模式。
; x64compatible 需要 Inno Setup 6.3+；它允许在 ARM64 Windows 上安装 x64 构建。
ArchitecturesAllowed=x64compatible
VersionInfoCompany={#AppPublisher}
VersionInfoDescription={#AppDisplayName} Setup
VersionInfoProductName={#AppDisplayName}
VersionInfoProductVersion={#AppVersion}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "附加任务："; Flags: unchecked

[Files]
Source: "{#SourceDir}\{#AppExeName}"; DestDir: "{app}"; Flags: ignoreversion
; CLI 必须与 GUI 同目录：mns-gui 通过自身所在目录定位 mns.exe 并把它交给外部 Agent。
Source: "{#SourceDir}\{#CliExeName}"; DestDir: "{app}"; Flags: ignoreversion
; 帮助中心与 GUI 同目录：托盘菜单「帮助中心」从自身所在目录启动 mns-help.exe。
Source: "{#SourceDir}\{#HelpExeName}"; DestDir: "{app}"; Flags: ignoreversion
; 管家审计 CLI（mns-butler status / wake / memory …）。
Source: "{#SourceDir}\{#ButlerExeName}"; DestDir: "{app}"; Flags: ignoreversion
; 管家守护进程：GUI 从自身所在目录拉起它，关掉主窗口后继续判定。它是控制台
; 程序，由带 CREATE_NO_WINDOW 的 GUI 启动，不会闪黑框。
Source: "{#SourceDir}\{#ButlerdExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
; 帮助截图（可选目录，没有图片时占位存在但为空，Inno 不会报错）。
Source: "{#SourceDir}\help\*"; DestDir: "{app}\help"; Flags: ignoreversion recursesubdirs createallsubdirs skipifsourcedoesntexist

[Icons]
Name: "{autoprograms}\{#AppDisplayName}"; Filename: "{app}\{#AppExeName}"; IconFilename: "{app}\{#AppExeName}"; WorkingDir: "{app}"
; 帮助中心的图标来自 mns-help.exe 自身嵌入的 ID-1 资源（build.rs 按 bin 编入
; icon_help.ico），所以快捷方式直接引用 exe，不再复用主程序的 app.ico。
Name: "{autoprograms}\{#AppDisplayName} 帮助中心"; Filename: "{app}\{#HelpExeName}"; IconFilename: "{app}\{#HelpExeName}"; WorkingDir: "{app}"
Name: "{autodesktop}\{#AppDisplayName}"; Filename: "{app}\{#AppExeName}"; IconFilename: "{app}\{#AppExeName}"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExeName}"; Description: "启动 {#AppDisplayName}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; 守护进程是常驻的：不先停掉它，卸载时它占着自己的 exe，文件删不掉。
Filename: "{sys}\taskkill.exe"; Parameters: "/IM {#ButlerdExeName} /F"; Flags: runhidden; RunOnceId: "StopButlerDaemon"

[Code]
// 升级时守护进程可能正跑着 mns-butlerd.exe，Windows 会锁住文件导致覆盖失败。
// 安装前先请它退出；GUI 下次启动会按需重新拉起（这段空档里判定回到 GUI 进程内）。
procedure StopButlerDaemon();
var
  ResultCode: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/IM {#ButlerdExeName} /F', '',
       SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopButlerDaemon();
  Result := '';
end;

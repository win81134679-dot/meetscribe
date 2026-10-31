; 語音轉錄（MeetScribe）安裝程式（Inno Setup 6.3+）
; 由 build-installer.ps1 編譯：ISCC /DAppVersion=x.y.z /DReleaseDir=<target\release> MeetScribe.iss
; 捷徑交給安裝程式建：未簽章的 exe 自己建 .lnk 容易被防毒判成可疑行為
; ffmpeg 不打包（完整版每個 exe 2 百多 MB）；找不到時在最後一頁提供用 winget 安裝，程式會自己去 winget 的位置找

; 2026-10-31 使用者：「程式名稱改成語音轉錄」。只改看得到的名稱；安裝資料夾、AppId、檔名維持 MeetScribe，升級才接得上
#define AppName "語音轉錄"
#define DirName "MeetScribe"
#define ShortcutName "語音轉錄"
#define OldShortcutName "MeetScribe 會議逐字稿"
#define ExeName "meetscribe-app.exe"
#define CliName "meetscribe-cli.exe"
; 第一次發佈後不能再改：升級與解除安裝靠它找到舊的安裝
#define AppId "{{3DD3938D-23B9-4000-B12D-02F45EE908B8}"

#ifndef AppVersion
  #error 請用 build-installer.ps1 編譯（要傳 /DAppVersion=...）
#endif
#ifndef ReleaseDir
  #error 請用 build-installer.ps1 編譯（要傳 /DReleaseDir=...）
#endif
#ifndef OutputDir
  #define OutputDir "..\dist"
#endif

[Setup]
AppId={#AppId}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=win81134679-dot
AppPublisherURL=https://github.com/win81134679-dot/meetscribe
VersionInfoVersion={#AppVersion}
VersionInfoProductName={#AppName}
VersionInfoDescription={#AppName} 安裝程式
; 只裝給目前帳號：%LOCALAPPDATA%\Programs\MeetScribe，不需要系統管理員
DefaultDirName={autopf}\{#DirName}
PrivilegesRequired=lowest
DisableDirPage=yes
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#OutputDir}
OutputBaseFilename=MeetScribe-Setup-v{#AppVersion}
UninstallDisplayIcon={app}\{#ExeName}
UninstallDisplayName={#AppName}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
; 程式開著時請使用者先關（Restart Manager 會列出來讓他按「自動關閉」）
CloseApplications=yes
RestartApplications=no
; 自動更新（程式用 /VERYSILENT /SELFUPDATE=1 [/RELAUNCH=1] 叫起來，見 crates/meetscribe-app/src/updater.rs）：
; 程式持有這個 mutex，[Code] 的 InitializeSetup 先等它關掉

[Languages]
Name: "cht"; MessagesFile: "compiler:Default.isl,ChineseTraditional.isl"

[Tasks]
Name: "desktopicon"; Description: "在桌面建立「{#ShortcutName}」捷徑"; GroupDescription: "捷徑："

[Files]
Source: "{#ReleaseDir}\{#ExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#ReleaseDir}\{#CliName}"; DestDir: "{app}"; Flags: ignoreversion

[InstallDelete]
; 改名前的捷徑
Type: files; Name: "{autoprograms}\{#OldShortcutName}.lnk"
Type: files; Name: "{autodesktop}\{#OldShortcutName}.lnk"

[Icons]
Name: "{autoprograms}\{#ShortcutName}"; Filename: "{app}\{#ExeName}"; WorkingDir: "{app}"
Name: "{autodesktop}\{#ShortcutName}"; Filename: "{app}\{#ExeName}"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
; 找不到 ffmpeg 才出現；winget 會跳出主控台視窗下載（約 100 MB），裝完再開程式
Filename: "{cmd}"; Parameters: "/c winget install --id Gyan.FFmpeg -e --scope user --accept-source-agreements --accept-package-agreements"; \
  Description: "安裝 ffmpeg（處理音訊必需，用 winget 下載約 100 MB）"; Flags: postinstall waituntilterminated; Check: not FfmpegFound
Filename: "{app}\{#ExeName}"; Description: "立即啟動 {#ShortcutName}"; Flags: nowait postinstall skipifsilent
; 自動更新按「立即更新並重新開啟」時（/RELAUNCH=1）：靜默裝完把程式開回來
Filename: "{app}\{#ExeName}"; Flags: nowait; Check: ShouldRelaunch

; 會議資料在 %APPDATA%\MeetScribe，解除安裝時保留（金鑰在 Windows 認證管理員，也不動）

[Code]
const
  RunningMutex = 'MeetScribe-Running';

function IsSelfUpdate: Boolean;
begin
  Result := ExpandConstant('{param:SELFUPDATE|0}') = '1';
end;

function ShouldRelaunch: Boolean;
begin
  Result := IsSelfUpdate and (ExpandConstant('{param:RELAUNCH|0}') = '1');
end;

// 自動更新：程式剛叫我們起來就自己關掉，最多等 60 秒讓它放開 exe
function InitializeSetup: Boolean;
var
  I: Integer;
begin
  Result := True;
  if IsSelfUpdate then
  begin
    I := 0;
    while CheckForMutexes(RunningMutex) and (I < 120) do
    begin
      Sleep(500);
      I := I + 1;
    end;
  end;
end;

// 跟程式找 ffmpeg 的順序一致（audio::Ffmpeg::locate）：PATH，再來 winget 的套件資料夾
function HasFfmpegIn(Dir: String): Boolean;
begin
  Result := FileExists(AddBackslash(Dir) + 'ffmpeg.exe') and FileExists(AddBackslash(Dir) + 'ffprobe.exe');
end;

function FfmpegFound: Boolean;
var
  Paths, Dir, Pkgs: String;
  P: Integer;
  Rec, Inner: TFindRec;
begin
  Result := False;
  Paths := GetEnv('PATH') + ';';
  while (not Result) and (Paths <> '') do
  begin
    P := Pos(';', Paths);
    Dir := Trim(Copy(Paths, 1, P - 1));
    Delete(Paths, 1, P);
    if Dir <> '' then
      Result := HasFfmpegIn(Dir);
  end;
  if Result then Exit;
  Pkgs := ExpandConstant('{localappdata}\Microsoft\WinGet\Packages');
  if FindFirst(Pkgs + '\*FFmpeg*', Rec) then
  try
    repeat
      if FindFirst(Pkgs + '\' + Rec.Name + '\*', Inner) then
      try
        repeat
          if (Inner.Name <> '.') and (Inner.Name <> '..') then
            Result := HasFfmpegIn(Pkgs + '\' + Rec.Name + '\' + Inner.Name + '\bin');
        until Result or not FindNext(Inner);
      finally
        FindClose(Inner);
      end;
    until Result or not FindNext(Rec);
  finally
    FindClose(Rec);
  end;
end;

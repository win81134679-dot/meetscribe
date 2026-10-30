# 建置正式版安裝程式：測試 → release 建置 → Inno Setup 編譯 → dist\MeetScribe-Setup-v<版本>.exe
# 用法：powershell -ExecutionPolicy Bypass -File installer\build-installer.ps1
# 程式開著時 exe 被鎖住，先關掉 MeetScribe 再跑。這支只編譯，不執行安裝程式。

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

Push-Location $root
try {
    cargo test --workspace
    if ($LASTEXITCODE) { throw '測試失敗' }
    # 這台分頁檔小，偶爾第一次會撞 os error 1455；失敗就再試一次
    cargo build --release --workspace
    if ($LASTEXITCODE) { cargo build --release --workspace }
    if ($LASTEXITCODE) { throw '建置失敗' }
} finally { Pop-Location }

$version = (Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
$release = Join-Path $root 'target\release'

$iscc = @("$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe", "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe") |
    Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw '找不到 Inno Setup 6（winget install JRSoftware.InnoSetup）' }
& $iscc /Q "/DAppVersion=$version" "/DReleaseDir=$release" (Join-Path $PSScriptRoot 'MeetScribe.iss')
if ($LASTEXITCODE) { throw "安裝程式編譯失敗（ISCC exit $LASTEXITCODE）" }

$setup = Join-Path $root "dist\MeetScribe-Setup-v$version.exe"
Write-Host "完成：" -ForegroundColor Green
"  {0}  {1:N1} MB  SHA256 {2}" -f (Split-Path -Leaf $setup), ((Get-Item $setup).Length / 1MB), (Get-FileHash $setup -Algorithm SHA256).Hash

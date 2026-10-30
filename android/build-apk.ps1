# 建置 Android 正式版 APK：Rust 交叉編譯 → Gradle assembleRelease → 簽章檢查 → dist\MeetScribe-Android-v<版本>.apk
# 用法：powershell -ExecutionPolicy Bypass -File android\build-apk.ps1 [-Debug] [-Abis arm64-v8a,x86_64]
# -Abis 加 x86_64 是給電腦上的模擬器測試；發佈版只要 arm64-v8a（預設）。
#
# AGP 拒絕在含中文的路徑建置（aapt2、NDK 工具真的會壞），所以整個建置在 subst 掛出來的英數磁碟代號下跑，
# 結束就 subst /D（只拿掉代號，不動檔案）。簽章用 android\keystore.properties（不進版控，見 README）。
# 這支只編譯，不安裝、不執行 App。

param([switch]$Debug, [string]$Abis = 'arm64-v8a')
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

$env:JAVA_HOME = @($env:JAVA_HOME, 'C:\Program Files\Android\Android Studio\jbr') |
    Where-Object { $_ -and (Test-Path "$_\bin\java.exe") } | Select-Object -First 1
if (-not $env:JAVA_HOME) { throw '找不到 JDK（裝 Android Studio，或設定 JAVA_HOME）' }
$sdk = @($env:ANDROID_HOME, "$HOME\Android\Sdk", "$env:LOCALAPPDATA\Android\Sdk") |
    Where-Object { $_ -and (Test-Path "$_\platforms") } | Select-Object -First 1
if (-not $sdk) { throw '找不到 Android SDK' }
$env:ANDROID_HOME = $sdk

$keystore = Join-Path $PSScriptRoot 'keystore.properties'
if (-not $Debug -and -not (Test-Path $keystore)) {
    throw '沒有 android\keystore.properties：正式版需要簽章金鑰（產生方式見 README「Android 版」）'
}

$letter = [char[]]([int][char]'R'..[int][char]'Z') | ForEach-Object { "$($_):" } |
    Where-Object { -not (Test-Path "$_\") } | Select-Object -First 1
if (-not $letter) { throw '找不到空的磁碟代號可以給 subst 用' }
subst $letter $root
if ($LASTEXITCODE) { throw "subst $letter 失敗" }
try {
    Push-Location "$letter\android"
    try {
        $task = if ($Debug) { 'assembleDebug' } else { 'assembleRelease' }
        # 不留常駐 daemon：這台記憶體吃緊，而且 daemon 會一直佔住輸出管線，讓呼叫端等不到結束
        & .\gradlew.bat $task --console=plain --no-daemon "-Pmeetscribe.abis=$Abis"
        if ($LASTEXITCODE) { throw "Gradle $task 失敗" }
    } finally { Pop-Location }
} finally {
    subst $letter /D
}

$version = (Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
$variant = if ($Debug) { 'debug' } else { 'release' }
$built = Get-ChildItem (Join-Path $PSScriptRoot "app\build\outputs\apk\$variant") -Filter *.apk | Select-Object -First 1
if (-not $built) { throw '找不到產出的 APK' }

$buildTools = Get-ChildItem "$sdk\build-tools" -Directory | Sort-Object { [version]$_.Name } | Select-Object -Last 1
# 先收完整輸出再過濾：直接接 Select-Object -First 會提早關管線，apksigner 被中斷而回非 0
# JDK 25 會在 stderr 印 restricted method 警告；PS 5.1 在 Stop 模式下會把它當成錯誤中止，這一步暫時放寬
$ErrorActionPreference = 'Continue'
$verify = & "$($buildTools.FullName)\apksigner.bat" verify --print-certs $built.FullName 2>$null
$ErrorActionPreference = 'Stop'
if ($LASTEXITCODE) { throw 'APK 簽章驗證失敗' }
$verify | Where-Object { $_ -like 'Signer #1 certificate*' }

$dist = Join-Path $root 'dist'
New-Item -ItemType Directory -Force $dist | Out-Null
$suffix = if ($Debug) { '-debug' } else { '' }
$out = Join-Path $dist "MeetScribe-Android-v$version$suffix.apk"
Copy-Item $built.FullName $out -Force
Write-Host '完成：' -ForegroundColor Green
"  {0}  {1:N1} MB  SHA256 {2}" -f (Split-Path -Leaf $out), ((Get-Item $out).Length / 1MB), (Get-FileHash $out -Algorithm SHA256).Hash

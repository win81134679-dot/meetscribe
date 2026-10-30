# 交叉編譯 Rust 核心成 Android 原生函式庫，放進 app\src\main\jniLibs\<ABI>\libmeetscribe_android.so。
# 用法：powershell -ExecutionPolicy Bypass -File android\build-rust.ps1 [-Debug] [-Abi arm64-v8a,x86_64]
# Gradle 的 preBuild 也會自動叫這支（見 app\build.gradle.kts），平常不必手動跑。
# 正式版只有 arm64-v8a；x86_64 是給電腦上的模擬器測試用（gradlew -Pmeetscribe.abis=arm64-v8a,x86_64）。
#
# 需要：rustup 的 stable 工具鏈＋對應的 *-linux-android target、Android SDK 的 NDK。
# 這台的 PATH 先找到 MSI 版 rustc（沒有 Android 標準庫），所以明確指定 rustup 工具鏈的 cargo／rustc。

param([switch]$Debug, [string[]]$Abi = @('arm64-v8a'))
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
# Gradle 傳進來的是一個逗號分隔字串
$Abi = $Abi | ForEach-Object { $_ -split ',' } | ForEach-Object { $_.Trim() } | Where-Object { $_ }
$targets = @{ 'arm64-v8a' = 'aarch64-linux-android'; 'x86_64' = 'x86_64-linux-android' }
foreach ($a in $Abi) { if (-not $targets.ContainsKey($a)) { throw "不支援的 ABI：$a（可用：$($targets.Keys -join '、')）" } }

$toolchain = Join-Path $HOME '.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin'
if (-not (Test-Path "$toolchain\cargo.exe")) { throw "找不到 rustup stable 工具鏈：$toolchain（rustup toolchain install stable）" }
foreach ($a in $Abi) {
    if (-not (Test-Path "$toolchain\..\lib\rustlib\$($targets[$a])")) {
        & "$HOME\.cargo\bin\rustup.exe" target add $targets[$a] --toolchain stable
    }
}

$sdk = @($env:ANDROID_HOME, $env:ANDROID_SDK_ROOT, "$HOME\Android\Sdk", "$env:LOCALAPPDATA\Android\Sdk") |
    Where-Object { $_ -and (Test-Path "$_\ndk") } | Select-Object -First 1
if (-not $sdk) { throw '找不到 Android SDK 的 NDK（sdkmanager --install "ndk;28.2.13676358"）' }
$ndk = Get-ChildItem "$sdk\ndk" -Directory | Sort-Object { [version]$_.Name } | Select-Object -Last 1
$bin = Join-Path $ndk.FullName 'toolchains\llvm\prebuilt\windows-x86_64\bin'
$minApi = 26  # 跟 app\build.gradle.kts 的 minSdk 一致

$env:RUSTC = "$toolchain\rustc.exe"
$env:CARGO_PROFILE_RELEASE_STRIP = 'symbols'
# 不用 NDK 的 <target>26-clang.cmd：.cmd 經過 cmd.exe 會用 cp950 轉碼參數，
# 專案路徑裡的中文會變亂碼，lld 就找不到檔案。直接叫 clang.exe 並自己帶 --target。
foreach ($a in $Abi) {
    $t = $targets[$a]
    $upper = $t.ToUpper().Replace('-', '_')
    $under = $t.Replace('-', '_')
    $flag = "--target=$t$minApi"
    Set-Item "env:CARGO_TARGET_$($upper)_LINKER" "$bin\clang.exe"
    Set-Item "env:CARGO_TARGET_$($upper)_RUSTFLAGS" "-C link-arg=$flag"
    Set-Item "env:CC_$under" "$bin\clang.exe"
    Set-Item "env:CFLAGS_$under" $flag
    Set-Item "env:AR_$under" "$bin\llvm-ar.exe"
}

# 注意：if 運算式回傳單一字串，直接 splat 會被拆成一個個字元，所以用 [string[]] 固定成陣列
[string[]]$profileArgs = if ($Debug) { @() } else { @('--release') }
$profileDir = if ($Debug) { 'debug' } else { 'release' }
[string[]]$targetArgs = $Abi | ForEach-Object { '--target'; $targets[$_] }

# 連 clang.exe 本身也會把參數寫成 ANSI（cp950）回應檔再交給 ld.lld，中文路徑照樣變亂碼。
# 所以用 subst 把專案暫時掛成一個磁碟代號，在純英數路徑下建置，結束就卸掉（subst /D 只拿掉代號，不動檔案）。
# build-apk.ps1 已經掛好（路徑全是英數）就直接用。代號固定從 R: 起找，cargo 的增量快取才對得上同一個路徑。
$letter = $null
if ($root -match '[^\x00-\x7F]') {
    $letter = [char[]]([int][char]'R'..[int][char]'Z') | ForEach-Object { "$($_):" } |
        Where-Object { -not (Test-Path "$_\") } | Select-Object -First 1
    if (-not $letter) { throw '找不到空的磁碟代號可以給 subst 用' }
    subst $letter $root
    if ($LASTEXITCODE) { throw "subst $letter 失敗" }
}
Push-Location $(if ($letter) { "$letter\" } else { $root })
try {
    & "$toolchain\cargo.exe" build -p meetscribe-android @targetArgs @profileArgs
    # 這台分頁檔小，偶爾第一次會撞 os error 1455；失敗就再試一次
    if ($LASTEXITCODE) { & "$toolchain\cargo.exe" build -p meetscribe-android @targetArgs @profileArgs }
    if ($LASTEXITCODE) { throw 'Rust 交叉編譯失敗' }
} finally {
    Pop-Location
    if ($letter) { subst $letter /D }
}

foreach ($a in $Abi) {
    $so = Join-Path $root "target\$($targets[$a])\$profileDir\libmeetscribe_android.so"
    $dest = Join-Path $PSScriptRoot "app\src\main\jniLibs\$a"
    New-Item -ItemType Directory -Force $dest | Out-Null
    Copy-Item $so $dest -Force
    "{0}  {1:N1} MB" -f (Join-Path $dest 'libmeetscribe_android.so'), ((Get-Item $so).Length / 1MB)
}

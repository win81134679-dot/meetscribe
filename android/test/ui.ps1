# 模擬器 UI 小工具：ui.ps1 tap "<文字>" [第幾個] | shot <名稱> | find "<文字>" | text "<要輸入的字>" | key <keycode>
param([string]$Action, [string]$Arg, [int]$Index = 0)
$adb = "$HOME\Android\Sdk\platform-tools\adb.exe"
$shots = Join-Path $env:TEMP 'meetscribe-shots'  # 截圖放暫存區，不進專案

function Get-Nodes {
    & $adb shell uiautomator dump /sdcard/ui.xml | Out-Null
    [xml]$x = (& $adb shell cat /sdcard/ui.xml) -join ''
    $x.SelectNodes('//node')
}
function Center($n) {
    $m = [regex]::Match($n.bounds, '\[(\d+),(\d+)\]\[(\d+),(\d+)\]')
    $x = ([int]$m.Groups[1].Value + [int]$m.Groups[3].Value) / 2
    $y = ([int]$m.Groups[2].Value + [int]$m.Groups[4].Value) / 2
    "$([int]$x) $([int]$y)"
}

switch ($Action) {
    'tap' {
        $hits = @(Get-Nodes | Where-Object { $_.text -like "*$Arg*" -or $_.'content-desc' -like "*$Arg*" })
        if ($hits.Count -le $Index) { "找不到：$Arg"; exit 1 }
        $c = Center $hits[$Index]
        & $adb shell "input tap $c"
        "tap $Arg @ $c"
    }
    'find' {
        Get-Nodes | Where-Object { $_.text -like "*$Arg*" -or $_.'content-desc' -like "*$Arg*" } |
            ForEach-Object { "{0} | {1} | {2}" -f $_.text, $_.'content-desc', $_.bounds }
    }
    'text' { & $adb shell input text "'$Arg'" }
    'key' { & $adb shell input keyevent $Arg }
    'shot' {
        New-Item -ItemType Directory -Force $shots | Out-Null
        cmd /c "`"$adb`" exec-out screencap -p > `"$shots\$Arg.png`""
        "$shots\$Arg.png"
    }
}

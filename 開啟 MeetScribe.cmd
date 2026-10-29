@echo off
rem 雙擊開啟 MeetScribe（用的是 cargo build --release 產出的程式）
chcp 65001 >nul
set "APP=%~dp0target\release\meetscribe-app.exe"
if exist "%APP%" (
  start "" /D "%~dp0target\release" "%APP%"
  exit /b 0
)
echo 找不到 %APP%
echo 請先在這個資料夾執行：cargo build --release
pause
exit /b 1

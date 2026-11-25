@echo off
chcp 65001 >nul
setlocal enabledelayedexpansion
cd /d "%~dp0"

rem 用法：build.bat [nsis|msi|all]
rem   不带参数 = 只打 nsis（-setup.exe，单文件安装包，推荐分发用）
rem   msi        = 只打 msi（需要 WiX，首次会联网下载）
rem   all        = 两种都打

set "BUNDLE=%~1"
if "%BUNDLE%"=="" set "BUNDLE=nsis"

echo ============================================================
echo   Socket Tester 打包
echo   打包类型: %BUNDLE%   可选: nsis / msi / all
echo ============================================================
echo.

where node >nul 2>nul
if errorlevel 1 (
  echo [错误] 未找到 Node.js，请先安装并加入 PATH。
  goto :fail
)

where cargo >nul 2>nul
if errorlevel 1 (
  echo [错误] 未找到 Rust Cargo，请先安装 Rust 工具链。
  goto :fail
)

echo [1/2] 准备前端依赖 ...
if not exist "node_modules" (
  call npm install
  if errorlevel 1 goto :fail
) else (
  echo        node_modules 已存在，跳过 npm install。
)
echo.

echo [2/2] 编译并打包（release + LTO，首次约需数分钟）...
echo        开始时间: %DATE% %TIME%
echo.

call npm run tauri build -- --bundles %BUNDLE%
if errorlevel 1 goto :fail

echo.
echo        结束时间: %DATE% %TIME%
echo.
echo 产物：
if exist "src-tauri\target\release\socket-tester.exe" (
  echo   [可执行文件]  src-tauri\target\release\socket-tester.exe
)
if exist "src-tauri\target\release\bundle\msi\*.msi" (
  for %%F in ("src-tauri\target\release\bundle\msi\*.msi") do (
    echo   [MSI]         %%~fF
  )
)
if exist "src-tauri\target\release\bundle\nsis\*.exe" (
  for %%F in ("src-tauri\target\release\bundle\nsis\*.exe") do (
    echo   [安装包]      %%~fF
  )
)
echo.
echo 打包完成。
pause
exit /b 0

:fail
echo.
echo 打包失败，请查看上方日志。
echo 常见原因：网络无法下载 NSIS/WiX 打包工具，或 Rust 编译报错。
pause
exit /b 1

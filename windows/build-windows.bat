@echo off
setlocal
rem =====================================================================
rem  build-windows.bat -- double-click to build the Windows client exe.
rem  ASCII only on purpose: cmd.exe code-page issues must not break it.
rem
rem  It runs windows\scripts/build_windows.sh (the single owner of the
rem  build recipe) under Git Bash, then shows where the exe landed.
rem
rem  Needs:  Git for Windows (bash) + rustup with target
rem          x86_64-pc-windows-gnu
rem  Output: windows\dist\BenagenDownloader-<version>-Windows-x86_64.exe
rem          (version comes from the build script default; override by
rem           setting VERSION=... before double-clicking is not possible,
rem           so run the bash script directly if you need a custom one)
rem =====================================================================
cd /d "%~dp0.."

set "BASH_EXE="
where bash >nul 2>nul && set "BASH_EXE=bash"
if not defined BASH_EXE if exist "%ProgramFiles%\Git\bin\bash.exe" set "BASH_EXE=%ProgramFiles%\Git\bin\bash.exe"
if not defined BASH_EXE if exist "%ProgramFiles(x86)%\Git\bin\bash.exe" set "BASH_EXE=%ProgramFiles(x86)%\Git\bin\bash.exe"
if not defined BASH_EXE (
  echo [ERROR] bash.exe not found. Install "Git for Windows" first:
  echo         https://git-scm.com/download/win
  pause
  exit /b 1
)

if not exist "windows\scripts\build_windows.sh" (
  echo [ERROR] windows\scripts\build_windows.sh not found.
  echo         Run this from a full source checkout of the repo.
  pause
  exit /b 1
)

echo ============================================================
echo  Building the Windows client. It compiles code: 5-20 minutes.
echo  Console output is saved to: windows\build-windows.log
echo ============================================================
echo.

set "BUILDLOG=%~dp0build-windows.log"
"%BASH_EXE%" windows/scripts/build_windows.sh > "%BUILDLOG%" 2>&1
set ERR=%ERRORLEVEL%

type "%BUILDLOG%" 2>nul
echo.
echo ---- end of build log (exit code %ERR%) ----
echo.

if %ERR% neq 0 (
  echo ============================================================
  echo  [!!] Build FAILED. Please send us this file:
  echo       windows\build-windows.log
  echo ============================================================
) else (
  echo ============================================================
  echo  Build finished. The exe is in:
  dir /b "windows\dist\*.exe" 2>nul
  echo       ^(full path: %CD%\windows\dist\^)
  echo ============================================================
)
pause
exit /b %ERR%

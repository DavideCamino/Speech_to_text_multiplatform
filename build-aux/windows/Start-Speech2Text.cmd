@echo off
setlocal DisableDelayedExpansion
set "ROOT=%~dp0"
rem Do not depend on an MSYS2 installation or tools elsewhere on this machine.
set "PATH=%ROOT%bin;%SystemRoot%\System32;%SystemRoot%"
set "GTK_DATA_PREFIX=%ROOT%"
set "GTK_EXE_PREFIX=%ROOT%"
set "GTK_PATH="
set "XDG_DATA_DIRS=%ROOT%share"
set "GSETTINGS_SCHEMA_DIR=%ROOT%share\glib-2.0\schemas"
set "GSETTINGS_BACKEND=memory"
set "GIO_MODULE_DIR=%ROOT%lib\gio\modules"
set "GIO_EXTRA_MODULES="
set "GIO_USE_VFS=local"
set "GDK_BACKEND=win32"
set "GSK_RENDERER=cairo"
set "GTK_USE_PORTAL=0"
set "FONTCONFIG_FILE="
set "FONTCONFIG_PATH="
if exist "%ROOT%etc\fonts\fonts.conf" (
  set "FONTCONFIG_FILE=%ROOT%etc\fonts\fonts.conf"
  set "FONTCONFIG_PATH=%ROOT%etc\fonts"
)
if not exist "%ROOT%bin\speech2text.exe" goto setup_failed
if not exist "%GSETTINGS_SCHEMA_DIR%\gschemas.compiled" goto setup_failed
if not defined LOCALAPPDATA set "LOCALAPPDATA=%TEMP%"
set "CACHE=%LOCALAPPDATA%\io.github.davidecamino.Speech2Text\cache"
if not exist "%CACHE%" mkdir "%CACHE%"
set "GDK_PIXBUF_MODULEDIR=%ROOT%lib\gdk-pixbuf-2.0\2.10.0\loaders"
set "GDK_PIXBUF_MODULE_FILE=%CACHE%\loaders.cache"
rem Loader caches contain absolute DLL paths. Recreate after every move/update,
rem in the user's writable cache, not in the (possibly read-only) app directory.
"%ROOT%bin\gdk-pixbuf-query-loaders.exe" > "%GDK_PIXBUF_MODULE_FILE%"
if not "%ERRORLEVEL%"=="0" goto setup_failed
cd /d "%ROOT%"
"%ROOT%bin\speech2text.exe" %*
set "APP_EXIT=%ERRORLEVEL%"
if not "%APP_EXIT%"=="0" goto app_failed
exit /b 0

:setup_failed
echo Speech2Text runtime setup failed. Extract the entire ZIP before launching.
echo Keep bin, lib, share, and this launcher together.
if not defined SPEECH2TEXT_NO_PAUSE pause
exit /b 1

:app_failed
echo Speech2Text exited with code %APP_EXIT%.
echo Please include the messages above in your issue report.
if not defined SPEECH2TEXT_NO_PAUSE pause
exit /b %APP_EXIT%

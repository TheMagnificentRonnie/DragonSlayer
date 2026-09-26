@echo off
rem Kill any stale instance so we're always running the freshest build.
taskkill /IM dragonslayer-app.exe /F >nul 2>&1
set "CAMLIBS=C:\msys64\ucrt64\lib\libgphoto2\2.5.34"
set "IOLIBS=C:\msys64\ucrt64\lib\libgphoto2_port\0.12.2"
start "" "%~dp0..\target\x86_64-pc-windows-gnu\release\dragonslayer-app.exe" %*

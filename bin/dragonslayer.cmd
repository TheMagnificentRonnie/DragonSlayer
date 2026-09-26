@echo off
set "CAMLIBS=C:\msys64\ucrt64\lib\libgphoto2\2.5.34"
set "IOLIBS=C:\msys64\ucrt64\lib\libgphoto2_port\0.12.2"
"%~dp0..\target\x86_64-pc-windows-gnu\release\dragonslayer.exe" %*

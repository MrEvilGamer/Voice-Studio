@echo off
rem Direct cl/link build for VoiceStudioMic.sys - no msbuild/VSIX needed.
setlocal enabledelayedexpansion

set KIT=C:\Program Files (x86)\Windows Kits\10
set VER=10.0.26100.0
set INC=%KIT%\Include\%VER%
set LIB=%KIT%\Lib\%VER%

set MSVCROOT=
for /f "usebackq delims=" %%i in (`"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set MSVCROOT=%%i
for /f "delims=" %%d in ('dir /b /ad "%MSVCROOT%\VC\Tools\MSVC" ^| sort') do set MSVCTOOLS=%MSVCROOT%\VC\Tools\MSVC\%%d

set PATH=%MSVCTOOLS%\bin\Hostx64\x64;%PATH%

set OUTDIR=%~dp0build\x64
if not exist "%OUTDIR%" mkdir "%OUTDIR%"

set CFLAGS=/c /Zp8 /GR- /GS /Gz /W3 /O2 /Oi /Oy- /kernel /Zc:alignedNew- /D_WIN64 /DWIN64 /D_AMD64_ /DAMD64 /D_KERNEL_MODE /DPOOL_NX_OPTIN=1 /DNTDDI_VERSION=0x0A00000B /D_WIN32_WINNT=0x0A00 /D_NO_CRT_STDIO_INLINE /D_NEW_DELETE_OPERATORS_
set INCLUDES=/I"%MSVCTOOLS%\include" /I"%INC%\km" /I"%INC%\shared" /I"%INC%\ucrt" /I"%KIT%\Include\wdf\kmdf\1.33" /I"%~dp0Source\Inc" /I"%~dp0Source\Filters" /I"%~dp0Source\Main" /I"%~dp0Source\Utilities"

set "SRCS="%~dp0Source\Main\adapter.cpp" "%~dp0Source\Main\basetopo.cpp" "%~dp0Source\Main\common.cpp" "%~dp0Source\Main\mintopo.cpp" "%~dp0Source\Main\minwavert.cpp" "%~dp0Source\Main\minwavertstream.cpp" "%~dp0Source\Main\NewDelete.cpp" "%~dp0Source\Main\vsmctrl.cpp" "%~dp0Source\Filters\micarraytopo.cpp" "%~dp0Source\Filters\speakertopo.cpp" "%~dp0Source\Utilities\hw.cpp" "%~dp0Source\Utilities\kshelper.cpp" "%~dp0Source\Utilities\savedata.cpp" "%~dp0Source\Utilities\ToneGenerator.cpp""

set LINKFLAGS=/OUT:"%OUTDIR%\VoiceStudioMic.sys" /MACHINE:X64 /PDB:"%OUTDIR%\VoiceStudioMic.pdb" /DRIVER /SUBSYSTEM:NATIVE,10.00 /ENTRY:GsDriverEntry /INTEGRITYCHECK /NODEFAULTLIB

pushd "%OUTDIR%"
for %%s in (%SRCS%) do (
  echo CL %%~nxs
  cl %CFLAGS% %INCLUDES% /Fo"%%~nxs.obj" %%s || goto :fail
)
set WDFLIB=%KIT%\Lib\wdf\kmdf\x64\1.33
echo LINK
link %LINKFLAGS% *.obj "%LIB%\km\x64\ntoskrnl.lib" "%LIB%\km\x64\hal.lib" "%LIB%\km\x64\portcls.lib" "%LIB%\km\x64\stdunk.lib" "%LIB%\km\x64\libcntpr.lib" "%LIB%\km\x64\BufferOverflowK.lib" "%LIB%\km\x64\wdmsec.lib" "%WDFLIB%\wdfdriverentry.lib" "%WDFLIB%\wdfldr.lib" || goto :fail
popd
echo BUILD OK: %OUTDIR%\VoiceStudioMic.sys
exit /b 0

:fail
popd
echo BUILD FAILED
exit /b 1

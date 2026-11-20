@echo off
rem Validates VoiceStudioMic.inf and produces VoiceStudioMic.cat
rem (the .sys must sit next to the INF for signability validation)
copy /y "C:\Users\valen\Desktop\Voice Studio\driver\vsvmic\build\x64\VoiceStudioMic.sys" "C:\Users\valen\Desktop\Voice Studio\driver\vsvmic\VoiceStudioMic.sys"
"C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x86\Inf2Cat.exe" /driver:"C:\Users\valen\Desktop\Voice Studio\driver\vsvmic" /os:10_X64 /uselocaltime

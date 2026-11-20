@echo off
"%~dp0tunnel.exe" gateway start
exit /b %errorlevel%

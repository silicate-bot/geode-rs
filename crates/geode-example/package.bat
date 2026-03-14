@echo off
setlocal enabledelayedexpansion

where python >nul 2>&1
if %errorlevel% neq 0 (
    echo Python is not installed or not in the PATH. Please install Python.
    exit /b 1
)

python build.py %1

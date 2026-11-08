# Свой набор для сборки Android-приложения Моста — без Android Studio и Unity.
# Ставит в C:\Exerussus\Programs\Bridge\tools (рядом с project, вне git):
#   jdk17\          — Eclipse Temurin JDK 17 (adoptium.net)
#   gradle-8.11.1\  — Gradle (services.gradle.org)
#   android-sdk\    — Android command-line tools + platform 35, build-tools 35.0.0, NDK 27.0.12077973
# Запуск: build-android.cmd setup (или powershell -ExecutionPolicy Bypass -File android\setup-sdk.ps1).
# Уже стоящее не трогает — можно запускать повторно (докачает недостающее).
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'   # иначе Invoke-WebRequest в PowerShell 5 качает в десятки раз медленнее
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Tools = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\tools'))
$Dl = Join-Path $Tools '_downloads'
New-Item -ItemType Directory -Force -Path $Tools, $Dl | Out-Null
Write-Host "Folder: $Tools"

function Get-File($url, $name) {
    $out = Join-Path $Dl $name
    if (Test-Path $out) { Write-Host "  already downloaded: $name"; return $out }
    Write-Host "  downloading $url"
    Invoke-WebRequest -Uri $url -OutFile "$out.part" -UseBasicParsing
    Move-Item "$out.part" $out
    return $out
}

function Expand-To($zip, $dest) {
    $tmp = Join-Path $Dl ('x-' + [IO.Path]::GetFileNameWithoutExtension($zip))
    if (Test-Path $tmp) { Rename-Item $tmp ($tmp + '-old-' + (Get-Date -Format 'HHmmss')) }
    Expand-Archive -Path $zip -DestinationPath $tmp
    $inner = Get-ChildItem $tmp | Select-Object -First 1
    Move-Item $inner.FullName $dest
}

# 1. JDK 17
$Jdk = Join-Path $Tools 'jdk17'
if (-not (Test-Path (Join-Path $Jdk 'bin\java.exe'))) {
    Write-Host '[1/3] JDK 17...'
    $z = Get-File 'https://api.adoptium.net/v3/binary/latest/17/ga/windows/x64/jdk/hotspot/normal/eclipse?project=jdk' 'jdk17.zip'
    Expand-To $z $Jdk
} else { Write-Host '[1/3] JDK 17 - already installed' }
$env:JAVA_HOME = $Jdk

# 2. Gradle
$GradleVer = '8.11.1'
$Gradle = Join-Path $Tools "gradle-$GradleVer"
if (-not (Test-Path (Join-Path $Gradle 'bin\gradle.bat'))) {
    Write-Host "[2/3] Gradle $GradleVer..."
    $z = Get-File "https://services.gradle.org/distributions/gradle-$GradleVer-bin.zip" "gradle-$GradleVer-bin.zip"
    Expand-To $z $Gradle
} else { Write-Host "[2/3] Gradle $GradleVer - already installed" }

# 3. Android SDK (command-line tools -> sdkmanager)
$Sdk = Join-Path $Tools 'android-sdk'
$Cl = Join-Path $Sdk 'cmdline-tools\latest'
if (-not (Test-Path (Join-Path $Cl 'bin\sdkmanager.bat'))) {
    Write-Host '[3/3] Android command-line tools...'
    New-Item -ItemType Directory -Force -Path (Join-Path $Sdk 'cmdline-tools') | Out-Null
    $z = Get-File 'https://dl.google.com/android/repository/commandlinetools-win-11076708_latest.zip' 'cmdline-tools.zip'
    Expand-To $z $Cl
} else { Write-Host '[3/3] Android command-line tools - already installed' }

$sm = Join-Path $Cl 'bin\sdkmanager.bat'
Write-Host '  accepting Android SDK licenses...'
$yes = ("y`n" * 30)
$yes | & $sm "--sdk_root=$Sdk" --licenses | Out-Null
Write-Host '  installing platform 35, build-tools 35.0.0, platform-tools, NDK 27.0.12077973 (about 1.5 GB)...'
$yes | & $sm "--sdk_root=$Sdk" 'platforms;android-35' 'build-tools;35.0.0' 'platform-tools' 'ndk;27.0.12077973'
if ($LASTEXITCODE -ne 0) { throw "sdkmanager failed ($LASTEXITCODE)" }

foreach ($p in @("$Sdk\platforms\android-35", "$Sdk\build-tools\35.0.0", "$Sdk\ndk\27.0.12077973\toolchains", "$Jdk\bin\java.exe", "$Gradle\bin\gradle.bat")) {
    if (-not (Test-Path $p)) { throw "not installed: $p" }
}
Write-Host ''
Write-Host "Done. Android SDK: $Sdk"
Write-Host 'Now run: .\build-android.cmd'

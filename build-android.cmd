@echo off
rem Мост для Android: ядро (Rust, most-mobile) -> привязки Kotlin -> APK.
rem   build-android.cmd          - собрать APK (publish\android\most-<версия>.apk)
rem   build-android.cmd keygen   - один раз: создать ключ подписи в ..\keys
rem   build-android.cmd setup    - один раз: скачать свои JDK 17, Gradle, Android SDK и NDK в ..\tools
rem Иначе SDK, NDK, JDK и Gradle берутся из Android Studio или Unity (модуль Android Build Support).
rem Ещё нужно (один раз, PowerShell): rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
rem                                   cargo install cargo-ndk
setlocal EnableExtensions EnableDelayedExpansion
chcp 65001 >nul
cd /d "%~dp0"

set GRADLE_VER=8.11.1
set KEYS=%~dp0..\keys
set UAP=
set UGRADLE=

if /i "%~1"=="setup" (
  powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0android\setup-sdk.ps1"
  exit /b !errorlevel!
)

rem --- свой набор (build-android.cmd setup) в ..\tools ---
set "TOOLS=%~dp0..\tools"
if "%ANDROID_HOME%"=="" if exist "%TOOLS%\android-sdk\platforms" set "ANDROID_HOME=%TOOLS%\android-sdk"
if "%JAVA_HOME%"=="" if exist "%TOOLS%\jdk17\bin\java.exe" set "JAVA_HOME=%TOOLS%\jdk17"

rem --- Android SDK: задан явно -> свой -> Android Studio -> Unity ---
if "%ANDROID_HOME%"=="" if exist "%LOCALAPPDATA%\Android\Sdk\platforms" set "ANDROID_HOME=%LOCALAPPDATA%\Android\Sdk"
if "%ANDROID_HOME%"=="" call :unity
if "%ANDROID_HOME%"=="" if not "%UAP%"=="" set "ANDROID_HOME=%UAP%\SDK"
if "%ANDROID_HOME%"=="" (
  echo [ошибка] Не найден Android SDK. Проще всего поставить свой: .\build-android.cmd setup
  echo          Unity Hub -^> Installs -^> ваша версия -^> Add modules -^> Android Build Support ^(с SDK, NDK и OpenJDK^).
  echo          Или укажите путь вручную: $env:MOST_UNITY_ANDROID = "...\Editor\Data\PlaybackEngines\AndroidPlayer"
  goto fail
)
if not exist "%ANDROID_HOME%\platforms" (
  echo [ошибка] В "%ANDROID_HOME%" нет platforms - SDK неполный.
  goto fail
)

rem --- NDK: задан явно -> Unity -> последний в SDK\ndk ---
if "%ANDROID_NDK_HOME%"=="" if not "%UAP%"=="" if exist "%UAP%\NDK\toolchains" set "ANDROID_NDK_HOME=%UAP%\NDK"
if "%ANDROID_NDK_HOME%"=="" for /d %%n in ("%ANDROID_HOME%\ndk\*") do if exist "%%n\toolchains" set "ANDROID_NDK_HOME=%%n"
if "%ANDROID_NDK_HOME%"=="" (
  echo [ошибка] Не найден Android NDK. Unity Hub -^> Add modules -^> Android Build Support -^> Android SDK ^& NDK Tools.
  goto fail
)

rem --- JDK 17: задан явно -> Android Studio -> Unity ---
if "%JAVA_HOME%"=="" if exist "%ProgramFiles%\Android\Android Studio\jbr\bin\java.exe" set "JAVA_HOME=%ProgramFiles%\Android\Android Studio\jbr"
if "%JAVA_HOME%"=="" if not "%UAP%"=="" if exist "%UAP%\OpenJDK\bin\java.exe" set "JAVA_HOME=%UAP%\OpenJDK"
if "%JAVA_HOME%"=="" (
  echo [ошибка] Нет JDK 17: Unity Hub -^> Add modules -^> Android Build Support -^> OpenJDK, или задайте JAVA_HOME.
  goto fail
)

echo SDK:  %ANDROID_HOME%
echo NDK:  %ANDROID_NDK_HOME%
echo JDK:  %JAVA_HOME%

if /i "%~1"=="keygen" goto keygen

for /f "tokens=3" %%v in ('findstr /b /c:"version = " Cargo.toml') do set VER=%%~v
echo.
echo === Мост для Android %VER% ===

cargo ndk --version >nul 2>&1 || (
  echo [ошибка] Нет cargo-ndk. В PowerShell: cargo install cargo-ndk
  goto fail
)
rustup target list --installed | findstr /b /c:"aarch64-linux-android" >nul || (
  echo [ошибка] Нет целей Rust для Android. В PowerShell:
  echo          rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
  goto fail
)

echo.
echo [1/4] Ядро most-mobile под arm64-v8a, armeabi-v7a, x86_64...
rem без strip: из .so берутся описания для привязок; Gradle сам уберёт отладочное при упаковке
set CARGO_PROFILE_RELEASE_STRIP=false
cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 -o android\app\src\main\jniLibs build --release -p most-mobile || goto fail

echo.
echo [2/4] Привязки Kotlin ^(uniffi^)...
cargo run --release -q -p most-mobile --features cli --bin uniffi-bindgen -- generate --library target\aarch64-linux-android\release\libmost_mobile.so --language kotlin --out-dir android\app\src\main\java || goto fail
if not exist android\app\src\main\java\com\exerussus\most\core\most_mobile.kt (
  echo [ошибка] Привязки не появились в android\app\src\main\java\com\exerussus\most\core
  goto fail
)

echo.
echo [3/4] Gradle...
set "SDKP=%ANDROID_HOME:\=\\%"
>android\local.properties echo sdk.dir=!SDKP!
if not exist android\gradle\wrapper\gradle-wrapper.jar call :wrapper || goto fail
set TASK=assembleRelease
set OUT=android\app\build\outputs\apk\release\app-release.apk
if not exist "%KEYS%\most-android.properties" (
  echo [i] Ключа подписи нет ^(build-android.cmd keygen^) - собираю отладочную версию.
  set TASK=assembleDebug
  set OUT=android\app\build\outputs\apk\debug\app-debug.apk
)
pushd android
call gradlew.bat !TASK! --no-daemon "-PmostNdkPath=%ANDROID_NDK_HOME%"
set GR=!errorlevel!
popd
if not "!GR!"=="0" goto fail

echo.
echo [4/4] publish\android...
if not exist publish\android mkdir publish\android
copy /y "!OUT!" "publish\android\most-%VER%.apk" >nul || goto fail
echo.
echo Готово: publish\android\most-%VER%.apk
echo Поставить на телефон: перешлите файл ^(например, через "Отправку" моста^) и откройте его на телефоне.
exit /b 0

:unity
rem самая новая Unity с модулем Android: папки Unity Hub и его настройки (android\find-unity.ps1)
if not "%MOST_UNITY_ANDROID%"=="" (
  set "UAP=%MOST_UNITY_ANDROID%"
) else (
  for /f "usebackq tokens=1,* delims==" %%a in (`powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0android\find-unity.ps1"`) do (
    if "%%a"=="UAP" set "UAP=%%b"
    if "%%a"=="INFO" echo %%b
  )
)
if not "%UAP%"=="" (
  echo Unity: %UAP%
  if exist "%UAP%\Tools\gradle\bin\gradle.bat" set "UGRADLE=%UAP%\Tools\gradle\bin\gradle.bat"
)
exit /b 0

:wrapper
rem gradlew делается любым Gradle в пустой папке (свой Gradle Unity может быть старше нужного проекту),
rem а нужный Gradle %GRADLE_VER% gradlew потом скачает сам
set GB=
for /d %%d in ("%USERPROFILE%\.gradle\wrapper\dists\gradle-*-bin\*") do for /d %%g in ("%%d\gradle-*") do if exist "%%g\bin\gradle.bat" set "GB=%%g\bin\gradle.bat"
if "%GB%"=="" set "GB=%UGRADLE%"
for /d %%g in ("%TOOLS%\gradle-*") do if exist "%%g\bin\gradle.bat" set "GB=%%g\bin\gradle.bat"
if "%GB%"=="" (
  echo [ошибка] Не нашёл Gradle ни в Unity ^(AndroidPlayer\Tools\gradle^), ни в %USERPROFILE%\.gradle.
  echo          Откройте папку project\android в Android Studio один раз или поставьте Gradle и запустите снова.
  exit /b 1
)
set "WD=%TEMP%\most-gradle-wrapper"
if not exist "%WD%" mkdir "%WD%"
>"%WD%\settings.gradle" echo rootProject.name = 'wrapper'
call "%GB%" -p "%WD%" wrapper --gradle-version %GRADLE_VER% --no-daemon || exit /b 1
copy /y "%WD%\gradlew.bat" android\ >nul || exit /b 1
copy /y "%WD%\gradlew" android\ >nul
copy /y "%WD%\gradle\wrapper\gradle-wrapper.jar" android\gradle\wrapper\ >nul || exit /b 1
exit /b 0

:keygen
if exist "%KEYS%\most-android.jks" (
  echo Ключ уже есть: %KEYS%\most-android.jks - не трогаю.
  exit /b 0
)
if not exist "%KEYS%" mkdir "%KEYS%"
echo Ключ подписи приложения. Сохраните копию %KEYS% в надёжном месте:
echo без него обновление поверх установленного приложения невозможно.
set /p KP=Придумайте пароль ключа (не меньше 6 знаков): 
"%JAVA_HOME%\bin\keytool.exe" -genkeypair -keystore "%KEYS%\most-android.jks" -alias most -keyalg RSA -keysize 4096 -validity 36500 -storepass "!KP!" -keypass "!KP!" -dname "CN=Most, O=Exerussus" || goto fail
>"%KEYS%\most-android.properties" (
  echo storeFile=most-android.jks
  echo storePassword=!KP!
  echo keyAlias=most
  echo keyPassword=!KP!
)
echo Готово: %KEYS%\most-android.jks
exit /b 0

:fail
echo.
echo [X] Сборка не удалась. Пришлите вывод выше.
exit /b 1

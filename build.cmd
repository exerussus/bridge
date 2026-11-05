@echo off
rem Сборка «Моста» 3.0 на Windows: publish\Most.exe (окно приложения + значок у часов) и publish\bridge.exe (консоль, скрипты).
rem Нужно:  Rust — winget install Rustlang.Rustup   (установщик предложит Visual Studio Build Tools — согласиться)
rem         Node.js — winget install OpenJS.NodeJS.LTS   (для панели; без него берётся готовая ui\dist, если она есть)
rem Ключи:  build.cmd notest  — собрать без тестов
chcp 65001 >nul
cd /d "%~dp0"
where cargo >nul 2>nul || goto :norust

echo [1/4] Панель (ui)...
where npm >nul 2>nul || goto :nonpm
pushd ui
rem зависимости ставятся заново, если package-lock.json изменился с прошлой установки (новые пакеты в выпуске)
if not exist node_modules goto :npmci
fc /b package-lock.json node_modules\.bridge-lock >nul 2>nul && goto :npmok
:npmci
call npm ci --no-audit --no-fund || (popd & goto :fail)
copy /y package-lock.json node_modules\.bridge-lock >nul
:npmok
call npm run build || (popd & goto :fail)
popd
goto :rust

:nonpm
if exist ui\dist\index.html (
  echo       Node.js не найден — беру готовую ui\dist
  goto :rust
)
echo Не найден Node.js (npm), а готовой панели ui\dist нет. Установите:  winget install OpenJS.NodeJS.LTS
echo затем откройте новое окно консоли и запустите build.cmd ещё раз.
exit /b 1

:rust
echo [2/4] Программа...
cargo build --release -p bridge-node -p bridge-app -p most-relay -p most-net || goto :fail
if not exist publish mkdir publish
copy /y target\release\bridge.exe publish\bridge.exe >nul || goto :fail
copy /y target\release\bridge-app.exe "publish\Most.exe" >nul || goto :fail
echo       publish\Most.exe и publish\bridge.exe готовы
rem приложение «Сервер» (свой сервер-ретранслятор по мастер-ключу): интерфейс + служба most-relay.exe
if not exist publish\apps\most-relay-admin\svc mkdir publish\apps\most-relay-admin\svc
xcopy /e /i /y /q apps\relay-admin publish\apps\most-relay-admin >nul || goto :fail
copy /y target\release\most-relay.exe publish\apps\most-relay-admin\svc\most-relay.exe >nul || goto :fail
echo       publish\apps\most-relay-admin — приложение «Сервер» (положите папку в apps рядом с Most.exe)
rem своя локальная сеть: служба most-net.exe и драйвер адаптера wintun.dll (скачивается один раз, подпись WireGuard LLC проверяется)
copy /y target\release\most-net.exe publish\most-net.exe >nul || goto :fail
if not exist publish\wintun.dll call :wintun
if exist publish\wintun.dll echo       publish\most-net.exe и publish\wintun.dll — своя локальная сеть (положите рядом с Most.exe)

if /i "%~1"=="notest" goto :done
echo [3/4] Тесты (около минуты; брандмауэр может спросить про доступ — тесты работают только на 127.0.0.1)...
cargo test --release -p bridge-core -p bridge-turn -p bridge-node -p most-relay -p most-net > publish\tests.log 2>&1 || goto :testfail
echo       тесты прошли (подробно: publish\tests.log)

:done
echo [4/4] Готово.
echo.
echo Установка: закройте старый «Мост» (значок у часов → Выход), положите Most.exe и bridge.exe в его папку
echo (рядом с bridge.json) и запустите Most.exe. Настройки, соседи, история и папки подхватятся.
echo Приложение «Сервер»: папку publish\apps\most-relay-admin — в папку apps моста (мост подхватит её сам).
echo Сервер для VPS (Linux): relay\bin\most-relay — как поставить, в relay\README.md.
echo Своя локальная сеть: publish\most-net.exe и publish\wintun.dll — рядом с Most.exe.
exit /b 0

:testfail
echo.
findstr /c:"FAILED" /c:"panicked" /c:"test result" /c:"error" publish\tests.log
echo.
echo Программа собрана (publish\) и работает, но часть тестов не прошла.
echo Подробности — в publish\tests.log (Claude прочитает его сам).
exit /b 2

:norust
echo Не найден Rust (cargo). Установите:  winget install Rustlang.Rustup
echo затем откройте новое окно консоли и запустите build.cmd ещё раз.
exit /b 1

:fail
echo Сборка не удалась (код %errorlevel%).
exit /b 1

:wintun
echo       Скачиваю wintun.dll (драйвер адаптера своей сети, www.wintun.net)...
powershell -NoProfile -ExecutionPolicy Bypass -Command "$ErrorActionPreference='Stop'; $z=Join-Path $env:TEMP 'wintun-0.14.1.zip'; Invoke-WebRequest -UseBasicParsing 'https://www.wintun.net/builds/wintun-0.14.1.zip' -OutFile $z; $d=Join-Path $env:TEMP 'wintun-0.14.1'; if (Test-Path $d) { Remove-Item -Recurse -Force $d }; Expand-Archive $z $d; $f=Join-Path $d 'wintun\bin\amd64\wintun.dll'; $s=Get-AuthenticodeSignature $f; if ($s.Status -ne 'Valid' -or $s.SignerCertificate.Subject -notmatch 'WireGuard') { throw ('bad wintun.dll signature: ' + $s.Status + ' ' + $s.SignerCertificate.Subject) }; Copy-Item $f 'publish\wintun.dll' -Force"
if errorlevel 1 echo       Не удалось скачать или проверить wintun.dll — своя сеть без неё не заработает (остальное собрано).
exit /b 0

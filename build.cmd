@echo off
rem Сборка «Моста» 3.0 на Windows: publish\Мост.exe (окно приложения + значок у часов) и publish\bridge.exe (консоль, скрипты).
rem Нужно:  Rust — winget install Rustlang.Rustup   (установщик предложит Visual Studio Build Tools — согласиться)
rem         Node.js — winget install OpenJS.NodeJS.LTS   (для панели; без него берётся готовая ui\dist, если она есть)
rem Ключи:  build.cmd notest  — собрать без тестов
chcp 65001 >nul
cd /d "%~dp0"
where cargo >nul 2>nul || goto :norust

echo [1/4] Панель (ui)...
where npm >nul 2>nul || goto :nonpm
pushd ui
if not exist node_modules (
  call npm ci --no-audit --no-fund || (popd & goto :fail)
)
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
cargo build --release -p bridge-node -p bridge-app || goto :fail
if not exist publish mkdir publish
copy /y target\release\bridge.exe publish\bridge.exe >nul || goto :fail
copy /y target\release\bridge-app.exe "publish\Мост.exe" >nul || goto :fail
echo       publish\Мост.exe и publish\bridge.exe готовы

if /i "%~1"=="notest" goto :done
echo [3/4] Тесты (около минуты; брандмауэр может спросить про доступ — тесты работают только на 127.0.0.1)...
cargo test --release -p bridge-core -p bridge-node > publish\tests.log 2>&1 || goto :testfail
echo       тесты прошли (подробно: publish\tests.log)

:done
echo [4/4] Готово.
echo.
echo Установка: закройте старый «Мост» (значок у часов → Выход), положите Мост.exe и bridge.exe в его папку
echo (рядом с bridge.json) и запустите Мост.exe. Настройки, соседи, история и папки подхватятся.
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

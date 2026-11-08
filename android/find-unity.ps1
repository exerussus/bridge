# Найти папку AndroidPlayer самой новой Unity с модулем Android Build Support.
# Смотрит: стандартную папку Unity Hub, свою папку установки Hub (secondaryInstallPath.json)
# и редакторы, добавленные в Hub вручную (editors-v2.json / editors.json).
# Печатает строки INFO=… (что нашёл) и UAP=<путь> (лучшая Unity с полным модулем Android).
$ErrorActionPreference = 'SilentlyContinue'
$roots = @()
$roots += (Join-Path $env:ProgramFiles 'Unity\Hub\Editor')
$hub = Join-Path $env:APPDATA 'UnityHub'
$sec = Join-Path $hub 'secondaryInstallPath.json'
if (Test-Path $sec) {
    try { $x = Get-Content $sec -Raw | ConvertFrom-Json; if ($x) { $roots += [string]$x } } catch {}
}
$editors = @()
foreach ($r in $roots) {
    if ($r -and (Test-Path $r)) { $editors += Get-ChildItem $r -Directory | ForEach-Object { $_.FullName } }
}
foreach ($f in @('editors-v2.json', 'editors.json')) {
    $p = Join-Path $hub $f
    if (-not (Test-Path $p)) { continue }
    try {
        $j = Get-Content $p -Raw | ConvertFrom-Json
        $items = @()
        if ($j.data) { $items = $j.data } else { $items = $j.PSObject.Properties | ForEach-Object { $_.Value } }
        foreach ($it in $items) {
            foreach ($loc in @($it.location)) {
                if (-not $loc) { continue }
                # location — путь к Unity.exe: <версия>\Editor\Unity.exe
                $root = Split-Path (Split-Path ([string]$loc) -Parent) -Parent
                if ($root) { $editors += $root }
            }
        }
    } catch {}
}
$best = $null
foreach ($e in ($editors | Select-Object -Unique | Sort-Object)) {
    $a = Join-Path $e 'Editor\Data\PlaybackEngines\AndroidPlayer'
    $parts = @()
    if (-not (Test-Path $a)) { $state = 'no Android Build Support' }
    else {
        if (-not (Test-Path (Join-Path $a 'SDK\platforms'))) { $parts += 'Android SDK and NDK Tools' }
        if (-not (Test-Path (Join-Path $a 'NDK'))) { if ($parts -notcontains 'Android SDK and NDK Tools') { $parts += 'Android SDK and NDK Tools' } }
        if (-not (Test-Path (Join-Path $a 'OpenJDK\bin\java.exe'))) { $parts += 'OpenJDK' }
        if ($parts.Count -eq 0) { $state = 'Android OK'; $best = $a }
        else { $state = 'Android Build Support without: ' + ($parts -join ', ') }
    }
    'INFO=Unity ' + (Split-Path $e -Leaf) + ': ' + $state
}
if (-not $editors) { 'INFO=Unity Hub: no editors found (looked in ' + ($roots -join '; ') + ' and ' + $hub + ')' }
if ($best) { 'UAP=' + $best }

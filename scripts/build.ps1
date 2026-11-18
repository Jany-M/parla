<#
.SYNOPSIS
  Builds the Parla Windows installer (NSIS .exe).

.EXAMPLE
  .\scripts\build.ps1                # tests + installer
  .\scripts\build.ps1 -SkipTests     # installer only (faster)
  .\scripts\build.ps1 -TestsOnly     # run all test suites, no installer
  .\scripts\build.ps1 -Clean         # wipe build output first

Missing Rust / Node / VS C++ Build Tools are installed automatically via winget when possible.
WebView2 is already on most Windows 10/11 PCs. NSIS is downloaded by Tauri.

The installer is copied to <project>\release\.
Build output lives in %LOCALAPPDATA%\parla-build (outside Dropbox, so syncing does not slow or lock the build).
#>
[CmdletBinding()]
param(
  [switch]$SkipTests,
  [switch]$TestsOnly,
  [switch]$SkipNpm,
  [switch]$Clean
)

$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..')
Set-Location $root

function Step($msg) { Write-Host "`n==> $msg" -ForegroundColor Cyan }
function Need($cmd, $hint) {
  if (-not (Get-Command $cmd -ErrorAction SilentlyContinue)) { throw "'$cmd' not found. $hint" }
}
function Run($exe, [string[]]$arguments) {
  & $exe @arguments
  if ($LASTEXITCODE -ne 0) { throw "'$exe $($arguments -join ' ')' failed (exit code $LASTEXITCODE)" }
}

# Merge Machine + User PATH into this process so winget installs are visible without a new shell.
function Refresh-Path {
  $machine = [Environment]::GetEnvironmentVariable('Path', 'Machine')
  $user = [Environment]::GetEnvironmentVariable('Path', 'User')
  $env:Path = (@($machine, $user) | Where-Object { $_ }) -join ';'
  $cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
  if (Test-Path $cargoBin) { $env:Path = "$cargoBin;$env:Path" }
}

Step 'Checking prerequisites'
Refresh-Path
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
function HasVsCpp {
  (Test-Path $vswhere) -and [bool](& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)
}
$install = @()
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { $install += @{ n = 'Rust'; a = @('install', '--id', 'Rustlang.Rustup', '-e', '--accept-source-agreements', '--accept-package-agreements') } }
if (-not (Get-Command node -ErrorAction SilentlyContinue))  { $install += @{ n = 'Node.js'; a = @('install', '--id', 'OpenJS.NodeJS.LTS', '-e', '--accept-source-agreements', '--accept-package-agreements') } }
if (-not (HasVsCpp)) { $install += @{ n = 'VS Build Tools (C++)'; a = @('install', '--id', 'Microsoft.VisualStudio.2022.BuildTools', '-e', '--accept-source-agreements', '--accept-package-agreements', '--override', '--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive --norestart') } }
if ($install.Count -gt 0) {
  Need winget 'Install Rust (rustup.rs), Node.js (nodejs.org) and VS Build Tools with the C++ workload manually.'
  Write-Host ('Missing: ' + (($install | ForEach-Object { $_.n }) -join ', ') + ' - installing with winget...') -ForegroundColor Yellow
  foreach ($i in $install) {
    Step "Installing $($i.n)"
    & winget @($i.a)
    if ($LASTEXITCODE -ne 0) { throw "Installing $($i.n) failed (exit $LASTEXITCODE)" }
  }
  Refresh-Path
  # rustup.exe often needs a first-run to place cargo on PATH
  if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    $rustup = Join-Path $env:USERPROFILE '.cargo\bin\rustup.exe'
    if (-not (Test-Path $rustup)) {
      $cmd = Get-Command rustup -ErrorAction SilentlyContinue
      if ($cmd) { $rustup = $cmd.Source }
    }
    if ($rustup) {
      Step 'Initializing Rust toolchain'
      & $rustup @('default', 'stable')
      Refresh-Path
    }
  }
  if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw 'Rust installed but cargo is not on PATH yet. Close this window, open a new PowerShell, and run .\scripts\build.ps1 again.'
  }
  if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
    throw 'Node.js installed but node is not on PATH yet. Close this window, open a new PowerShell, and run .\scripts\build.ps1 again.'
  }
  if (-not (HasVsCpp)) {
    throw 'VS Build Tools installed but the C++ workload is not visible yet. Reboot or open a new shell, then run .\scripts\build.ps1 again.'
  }
  Write-Host 'Prerequisites ready; continuing build.' -ForegroundColor Green
}
Run rustup @('target', 'add', 'x86_64-pc-windows-msvc')

# keep build output out of Dropbox
if (-not $env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR = Join-Path $env:LOCALAPPDATA 'parla-build\target' }
if ($Clean -and (Test-Path $env:CARGO_TARGET_DIR)) { Step 'Cleaning'; Remove-Item -Recurse -Force $env:CARGO_TARGET_DIR }
if ($Clean -and (Test-Path "$root\dist")) { Remove-Item -Recurse -Force "$root\dist" }

if ($SkipNpm -and (Test-Path (Join-Path $root 'node_modules'))) {
  Step 'Skipping npm install (node_modules already present)'
} else {
  Step 'Installing JavaScript dependencies'
  # node_modules lives inside the project; Dropbox ignores it if you mark it (Dropbox > Selective sync / `Set-Content -Stream com.dropbox.ignored`)
  & npm ci
  if ($LASTEXITCODE -ne 0) { Write-Host 'Lock file out of sync, using npm install instead.' -ForegroundColor Yellow; Run npm @('install') }
  try { Set-Content -Path "$root\node_modules" -Stream com.dropbox.ignored -Value 1 -ErrorAction Stop } catch {}
}

if (-not $SkipTests) {
  Step 'Rust tests (parla-core)'
  Run cargo @('test', '-p', 'parla-core', '--lib')
  Step 'Frontend type-check and tests'
  Run npm @('run', 'typecheck')
  Run npm @('test')
}
if ($TestsOnly) { Write-Host "`nAll tests passed." -ForegroundColor Green; return }

Step 'Building installer (first build downloads and compiles a lot: 10-25 minutes)'
Run npm @('run', 'tauri', 'build')

$bundle = Join-Path $env:CARGO_TARGET_DIR 'release\bundle\nsis'
$installer = Get-ChildItem $bundle -Filter '*-setup.exe' -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $installer) { throw "Build finished but no installer found in $bundle" }

$out = Join-Path $root 'release'
New-Item -ItemType Directory -Force $out | Out-Null
Copy-Item $installer.FullName $out -Force
Write-Host "`nDone: $(Join-Path $out $installer.Name)" -ForegroundColor Green
Write-Host 'Install it, then uninstall from Windows Settings > Apps: that removes the app, its downloaded models, meetings and settings.'

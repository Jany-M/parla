<#
.SYNOPSIS
  Patch-bumps the app version across Cargo workspace, package.json, and tauri.conf.json.

.EXAMPLE
  .\scripts\bump-version.ps1              # 0.1.0 -> 0.1.1
  .\scripts\bump-version.ps1 -DryRun
#>
[CmdletBinding()]
param(
  [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..')

function Read-CurrentVersion {
  $cargo = Get-Content (Join-Path $root 'Cargo.toml') -Raw
  if ($cargo -notmatch '(?m)^version\s*=\s*"(\d+)\.(\d+)\.(\d+)"') {
    throw 'Could not find workspace version in Cargo.toml'
  }
  return @{
    Major = [int]$Matches[1]
    Minor = [int]$Matches[2]
    Patch = [int]$Matches[3]
    Text  = "$($Matches[1]).$($Matches[2]).$($Matches[3])"
  }
}

function Write-Versions([string]$from, [string]$to) {
  $cargoPath = Join-Path $root 'Cargo.toml'
  $pkgPath = Join-Path $root 'package.json'
  $tauriPath = Join-Path $root 'src-tauri\tauri.conf.json'

  $cargo = Get-Content $cargoPath -Raw
  $cargo2 = [regex]::Replace(
    $cargo,
    '(?m)(^\[workspace\.package\][\s\S]*?^version\s*=\s*")' + [regex]::Escape($from) + '(")',
    ('${1}' + $to + '${2}'),
    1
  )
  if ($cargo2 -eq $cargo) {
    # Fallback: first top-level version = under workspace.package is usually the only one at root
    $cargo2 = [regex]::Replace($cargo, '(?m)^(version\s*=\s*")' + [regex]::Escape($from) + '(")', ('${1}' + $to + '${2}'), 1)
  }
  if ($cargo2 -eq $cargo) { throw "Failed to update Cargo.toml version $from -> $to" }

  $pkg = Get-Content $pkgPath -Raw
  $pkg2 = [regex]::Replace($pkg, '("version"\s*:\s*")' + [regex]::Escape($from) + '(")', ('${1}' + $to + '${2}'), 1)
  if ($pkg2 -eq $pkg) { throw "Failed to update package.json version $from -> $to" }

  $tauri = Get-Content $tauriPath -Raw
  $tauri2 = [regex]::Replace($tauri, '("version"\s*:\s*")' + [regex]::Escape($from) + '(")', ('${1}' + $to + '${2}'), 1)
  if ($tauri2 -eq $tauri) { throw "Failed to update tauri.conf.json version $from -> $to" }

  if ($DryRun) {
    Write-Host "[dry-run] would bump $from -> $to"
    return
  }

  $utf8 = New-Object System.Text.UTF8Encoding $false
  [System.IO.File]::WriteAllText($cargoPath, $cargo2, $utf8)
  [System.IO.File]::WriteAllText($pkgPath, $pkg2, $utf8)
  [System.IO.File]::WriteAllText($tauriPath, $tauri2, $utf8)

  # Keep Cargo.lock workspace package versions aligned (no dependency bumps).
  Sync-CargoLockVersions -from $from -to $to
}

function Sync-CargoLockVersions([string]$from, [string]$to) {
  $lockPath = Join-Path $root 'Cargo.lock'
  if (-not (Test-Path $lockPath)) { return }
  $lock = Get-Content $lockPath -Raw
  # Only the path crates we publish as the app (avoid touching unrelated crates named similarly).
  foreach ($name in @('parla', 'parla-core')) {
    $pattern = '(?m)(^name = "' + [regex]::Escape($name) + '"\r?\nversion = ")' + [regex]::Escape($from) + '(")'
    $lock2 = [regex]::Replace($lock, $pattern, ('${1}' + $to + '${2}'), 1)
    if ($lock2 -eq $lock) {
      Write-Warning "Cargo.lock had no version $from for $name (already synced?)"
    }
    $lock = $lock2
  }
  $utf8 = New-Object System.Text.UTF8Encoding $false
  [System.IO.File]::WriteAllText($lockPath, $lock, $utf8)
}

$cur = Read-CurrentVersion
$nextPatch = $cur.Patch + 1
$next = "$($cur.Major).$($cur.Minor).$nextPatch"
Write-Versions -from $cur.Text -to $next

# GitHub Actions step outputs
if ($env:GITHUB_OUTPUT) {
  Add-Content -Path $env:GITHUB_OUTPUT -Value "previous=$($cur.Text)"
  Add-Content -Path $env:GITHUB_OUTPUT -Value "next=$next"
}

if (-not $DryRun) {
  Write-Host "Bumped $($cur.Text) -> $next"
}

<#
.SYNOPSIS
  Keeps the debug symbols for a build of Pip, so a crash report from it can still be read later.

.DESCRIPTION
  Windows puts debug symbols in a separate pip.pdb, eleven megabytes against Pip's two, so it is
  never shipped. But target\release\pip.pdb is overwritten by the next build, and a .pdb only
  matches the exact binary it was built with. Once a build has gone to anybody, its symbols are
  either kept now or gone for good.

  This builds release from a clean tree, stamps the binary with the commit it was built from
  (PIP_BUILD, which appears in every crash report), and copies the executable and its symbols into
  a folder named after that commit, with a manifest recording what it is.

  The archive lives outside the repository by default: it is binary, it is large, and nothing in
  git should be able to delete it. The default is <drive of the repo>\pip-symbols rather than a
  folder beside the repo, because the folder holding this project denies creating anything new in
  it, and because the project's own drive is the one with room for it.

  Usage:  powershell -ExecutionPolicy Bypass -File tools\archive-symbols.ps1 [-To <dir>] [-Force]
#>
param(
    [string]$To,
    [switch]$Force
)

$ErrorActionPreference = "Stop"
$repo = Resolve-Path "$PSScriptRoot\.."
if (-not $To) { $To = Join-Path ([System.IO.Path]::GetPathRoot($repo)) "pip-symbols" }

# A dirty tree cannot be archived honestly: the commit would name source that is not what was
# built, which is worse than having no symbols at all, because it looks authoritative.
$dirty = & git -C $repo status --porcelain
if ($dirty -and -not $Force) {
    Write-Host "Working tree has uncommitted changes, so the commit would not describe this build:" -ForegroundColor Yellow
    $dirty | Select-Object -First 10 | ForEach-Object { Write-Host "  $_" }
    throw "Commit first, or pass -Force to archive it as <commit>-dirty."
}

$commit = (& git -C $repo rev-parse --short=10 HEAD).Trim()
$stamp = if ($dirty) { "$commit-dirty" } else { $commit }

Write-Host "Building release as $stamp ..."
$env:PIP_BUILD = $stamp
& cargo build --release --manifest-path "$repo\Cargo.toml"
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$exe = "$repo\target\release\pip.exe"
$pdb = "$repo\target\release\pip.pdb"
foreach ($f in @($exe, $pdb)) {
    if (-not (Test-Path $f)) { throw "missing $f - is debug = 1 still set in Cargo.toml?" }
}

$dest = Join-Path $To $stamp
if ((Test-Path $dest) -and -not $Force) { throw "$dest already exists; pass -Force to replace it." }
New-Item -ItemType Directory -Force -Path $dest | Out-Null
Copy-Item $exe $dest -Force
Copy-Item $pdb $dest -Force

$version = (Select-String -Path "$repo\Cargo.toml" -Pattern '^version' | Select-Object -First 1).Line
$manifest = [ordered]@{
    build     = $stamp
    commit    = (& git -C $repo rev-parse HEAD).Trim()
    subject   = (& git -C $repo log -1 --format=%s).Trim()
    archived  = (Get-Date).ToString("yyyy-MM-dd HH:mm:ss")
    version   = $version
    exe_bytes = (Get-Item $exe).Length
    exe_sha256 = (Get-FileHash $exe -Algorithm SHA256).Hash
    pdb_bytes = (Get-Item $pdb).Length
}
$manifest | ConvertTo-Json | Set-Content "$dest\manifest.json" -Encoding utf8

Write-Host ""
Write-Host "Archived $stamp -> $dest" -ForegroundColor Green
$manifest.GetEnumerator() | ForEach-Object { "{0,-11}{1}" -f $_.Key, $_.Value }
Write-Host ""
Write-Host "A crash report names this build on its first line. To read one, put that folder's"
Write-Host "pip.pdb beside a copy of its pip.exe and reproduce, or point a debugger at them."

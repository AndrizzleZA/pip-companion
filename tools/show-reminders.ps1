<#
.SYNOPSIS
  Plays every reminder, one at a time, with a line saying what to look for.

.DESCRIPTION
  The §68 gestures are chosen per reminder, per priority and per creature, and the whole point of
  them is how they look -- so they need watching, not asserting. This runs a *separate* companion
  with its own throw-away profile: Pip keys its instance on PIP_DATA_DIR, so this one lives
  happily beside the real one, talks to nothing it owns, and takes its profile to the grave.

  It needs a build with the developer commands in it, which the normal release has not got:

      cargo build --release --features devtools -p pip

  Usage:  powershell -ExecutionPolicy Bypass -File tools\show-reminders.ps1
          -Creature bunny          one of sprout bunny kit tuft pebble bloom moth
          -AllCreatures            the important-email reaction of all seven, in turn
          -Pause 6                 seconds to watch each one
          -Silent                  no sounds
#>
param(
    [string]$Exe = "$PSScriptRoot\..\target\release\pip.exe",
    [ValidateSet('sprout', 'bunny', 'kit', 'tuft', 'pebble', 'bloom', 'moth')]
    [string]$Creature = 'sprout',
    [switch]$AllCreatures,
    [double]$Pause = 6,
    [switch]$Silent
)

$ErrorActionPreference = "Stop"
$Exe = (Resolve-Path $Exe).Path
$data = Join-Path $env:TEMP ("pip-show-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Force $data | Out-Null
$env:PIP_DATA_DIR = $data

function Seed($who) {
    # Insistent, because this is a demonstration: it is the style that shows the whole gesture
    # rather than the quiet form of it.
    $s = @{
        pet    = @{ character = $who; attention = "insistent"; liveliness = "normal" }
        notify = @{ sound = (-not $Silent) }
    }
    ($s | ConvertTo-Json -Depth 4) | Out-File -FilePath (Join-Path $data "settings.json") -Encoding utf8
}

function Start-Pip {
    Start-Process $Exe -ArgumentList '--portable' | Out-Null
    Start-Sleep -Milliseconds 1200
}

function Stop-Pip {
    & $Exe --cmd quit | Out-Null
    Start-Sleep -Milliseconds 600
}

function Watch($what, $prio, $says) {
    Write-Host ""
    Write-Host "  $says" -ForegroundColor Cyan
    & $Exe --cmd "fire $what $prio" | Out-Null
    Start-Sleep -Seconds $Pause
}

try {
    if ($AllCreatures) {
        Write-Host ""
        Write-Host "The same important email, seven creatures." -ForegroundColor Green
        Write-Host "Four of them come at you; three stay put and knock at the screen." -ForegroundColor Gray
        foreach ($who in 'sprout', 'bunny', 'kit', 'tuft', 'pebble', 'bloom', 'moth') {
            Seed $who
            Start-Pip
            Watch 'mail' 'high' "$who -- an important email."
            Stop-Pip
        }
    } else {
        Seed $Creature
        Start-Pip
        Write-Host ""
        Write-Host "Watching: $Creature. Each one gets $Pause seconds." -ForegroundColor Green
        Write-Host "The bubble is the old part. The body is the new part." -ForegroundColor Gray

        Watch 'mail' 'high' "An email you asked for -- a sender on your list, or one you starred. Flinch, come at you, both arms, then stop dead and point."
        Watch 'mail' 'normal' "Mail something guessed at -- a subject keyword, or Gmail's own marker. Noticed and pointed at, but it stays where it is."
        Watch 'water' 'normal' "Drink some water. It looks for a glass, tips its head back over nothing, and gives up."
        Watch 'custom' 'normal' "Your own reminder. Restless: weight shifting, a foot going, a huff."
        Watch 'date' 'normal' "Somebody's birthday. Pleased, and it says so."
        Watch 'brief' 'normal' "The morning summary. It behaves like something that has just got up."
        Watch 'custom' 'low' "And the same reminder marked low. A glance at the screen, and nothing else at all."

        Write-Host ""
        Write-Host "Last one: left on screen so you can click it. Close the bubble when you are done." -ForegroundColor Green
        & $Exe --cmd "fire mail high" | Out-Null
        Read-Host "Press Enter to close this test companion"
    }
} finally {
    Stop-Pip
    Remove-Item -Recurse -Force $data -ErrorAction SilentlyContinue
    Write-Host "Test profile deleted. Your own Pip was never touched." -ForegroundColor Gray
}

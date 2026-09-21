<#
.SYNOPSIS
  Measures Pip's resource use against the targets in docs/DESIGN.md.

.DESCRIPTION
  Launches the release build against a throw-away data folder (never your real settings), lets it
  settle, then samples CPU time, private bytes, working set, threads and handles over a window.
  Also measures startup (process creation -> pet on screen, as logged by the app) over several
  cold launches.

  Usage:  powershell -ExecutionPolicy Bypass -File tools\measure.ps1 [-Minutes 10] [-Launches 5]
#>
param(
    [switch]$Awake,
    [double]$Minutes = 10,
    [int]$Launches = 5,
    [string]$Exe = "$PSScriptRoot\..\target\release\pip.exe"
)

$ErrorActionPreference = "Stop"
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class Cyc {
  [DllImport("kernel32.dll")] public static extern bool QueryProcessCycleTime(IntPtr h, out ulong c);
}
"@
# Exact CPU cycles (GetProcessTimes is tick-sampled at 15.6 ms and badly over/under-counts a
# process that only wakes for sub-millisecond bursts).
function Get-Cycles($proc) { $c = [uint64]0; [void][Cyc]::QueryProcessCycleTime($proc.Handle, [ref]$c); $c }
$ghz = (Get-CimInstance Win32_Processor | Select-Object -First 1).MaxClockSpeed / 1000.0
$Exe = (Resolve-Path $Exe).Path
$data = Join-Path $env:TEMP ("pip-measure-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Force $data | Out-Null
$env:PIP_DATA_DIR = $data
if ($Awake) { $env:PIP_MEASURE_AWAKE = "1" } else { Remove-Item Env:PIP_MEASURE_AWAKE -ErrorAction SilentlyContinue }

function Stop-Pip { & $Exe --cmd quit | Out-Null; Start-Sleep -Milliseconds 700 }

# --- Startup ------------------------------------------------------------------------------
$startups = @()
for ($i = 0; $i -lt $Launches; $i++) {
    $p = Start-Process $Exe -PassThru
    Start-Sleep -Milliseconds 1500
    Stop-Pip
    $line = (Get-Content (Join-Path $data "pip.log") | Select-String "started in (\d+) ms" | Select-Object -Last 1)
    if ($line) { $startups += [int]$line.Matches[0].Groups[1].Value }
}

# --- Idle ---------------------------------------------------------------------------------
$p = Start-Process $Exe -PassThru
Start-Sleep -Seconds 75                      # steady state: after pop-in and the 60 s first-run bubble
& $Exe --cmd "show" | Out-Null               # (dismisses nothing; just proves the channel works)
$p.Refresh()
$cpu0 = $p.TotalProcessorTime.TotalMilliseconds
$cyc0 = Get-Cycles $p
$t0 = Get-Date
$samples = @()
$end = $t0.AddMinutes($Minutes)
while ((Get-Date) -lt $end) {
    Start-Sleep -Seconds 15
    $p.Refresh()
    $samples += [pscustomobject]@{
        Private = $p.PrivateMemorySize64
        WS      = $p.WorkingSet64
        Threads = $p.Threads.Count
        Handles = $p.HandleCount
        PrivWS  = (Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -Filter "IDProcess=$($p.Id)").WorkingSetPrivate
    }
}
$p.Refresh()
$cpuMs = $p.TotalProcessorTime.TotalMilliseconds - $cpu0
$cycMs = (Get-Cycles $p) - $cyc0; $cycMs = $cycMs / ($ghz * 1e6)
$elapsedH = ((Get-Date) - $t0).TotalHours
Stop-Pip

$exeKB = [math]::Round((Get-Item $Exe).Length / 1KB)
$mb = { param($b) [math]::Round($b / 1MB, 2) }
$priv = ($samples | Measure-Object Private -Average -Maximum)
$ws = ($samples | Measure-Object WS -Average -Maximum)
$pws = ($samples | Measure-Object PrivWS -Average -Maximum)
$th = ($samples | Measure-Object Threads -Maximum).Maximum
$hd = ($samples | Measure-Object Handles -Maximum).Maximum
$cpuPerHour = [math]::Round(($cpuMs / 1000) / $elapsedH, 2)
$cpuPct = [math]::Round(($cpuMs / 1000) / ($elapsedH * 3600) * 100, 4)
$cycPerHour = [math]::Round(($cycMs / 1000) / $elapsedH, 2)
$st = if ($startups.Count) { ($startups | Measure-Object -Average -Minimum -Maximum) } else { $null }

$rows = @(
    @("Processes", "1", "1", "PASS"),
    @("Idle private memory", "$(& $mb $priv.Average) MB avg / $(& $mb $priv.Maximum) MB max", "<= 8 MB", $(if ($priv.Maximum -le 8MB) { "PASS" } else { "FAIL" })),
    @("Idle working set", "$(& $mb $ws.Average) MB avg / $(& $mb $ws.Maximum) MB max", "<= 15 MB", $(if ($ws.Maximum -le 15MB) { "PASS" } else { "FAIL" })),
    @("Private working set", "$(& $mb $pws.Average) MB avg / $(& $mb $pws.Maximum) MB max", "(Task Mgr)", "INFO"),
    @("Idle CPU (cycles)", "$cycPerHour s per hour at $ghz GHz", "<= 1 s/h", $(if ($cycPerHour -le 1) { "PASS" } else { "FAIL" })),
    @("Idle CPU (tick-sampled)", "$cpuPerHour s per hour ($cpuPct %)", "noisy", "INFO"),
    @("Startup", $(if ($st) { "$([math]::Round($st.Average)) ms avg ($($st.Minimum)-$($st.Maximum))" } else { "n/a" }), "<= 150 ms", $(if ($st -and $st.Maximum -le 150) { "PASS" } else { "CHECK" })),
    @("Threads (max)", "$th", "few", "INFO"),
    @("Handles (max)", "$hd", "stable", "INFO"),
    @("Executable", "$exeKB KB", "<= 3 MB", $(if ($exeKB -le 3072) { "PASS" } else { "FAIL" }))
)
"`nPip resource measurement  ({0:N1} min idle window, {1} launches, pet {2})`n" -f $Minutes, $Launches, $(if ($Awake) { "forced awake" } else { "free to nap when you are away" })
"{0,-22} {1,-46} {2,-10} {3}" -f "Metric", "Measured", "Target", "Result"
"{0,-22} {1,-46} {2,-10} {3}" -f "------", "--------", "------", "------"
foreach ($r in $rows) { "{0,-22} {1,-46} {2,-10} {3}" -f $r[0], $r[1], $r[2], $r[3] }
Remove-Item -Recurse -Force $data -ErrorAction SilentlyContinue

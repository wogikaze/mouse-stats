param(
    [ValidateRange(1, 86400)][int]$DurationSeconds = 1800,
    [ValidateRange(1, 60)][int]$IntervalSeconds = 5,
    [int]$ProcessId = 0,
    [string]$OutputPath = 'test-results/native-resources.json',
    [string]$DataDirectory = (Join-Path $env:APPDATA 'dev.wgkz.mouse-stats')
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# Read only: never starts collection, changes preferences or reads database contents.
# Counter definitions: https://learn.microsoft.com/windows/win32/cimwin32prov/win32-process
function Read-Processes {
    @(Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId, Name, CreationDate, KernelModeTime, UserModeTime, WorkingSetSize, PrivatePageCount, ReadTransferCount, WriteTransferCount)
}
function Get-DatabaseBytes {
    $total = 0L
    foreach ($name in @('statistics.db', 'statistics.db-wal', 'statistics.db-shm')) {
        $file = Get-Item -LiteralPath (Join-Path $DataDirectory $name) -ErrorAction SilentlyContinue
        if ($null -ne $file) { $total += $file.Length }
    }
    $total
}
$all = Read-Processes
if ($ProcessId -eq 0) {
    $matches = @($all | Where-Object Name -eq 'mouse-stats.exe')
    if ($matches.Count -ne 1) { throw 'Start exactly one Mouse Stats instance, or specify -ProcessId.' }
    $ProcessId = [int]$matches[0].ProcessId
}
$root = @($all | Where-Object ProcessId -eq $ProcessId)
if ($root.Count -ne 1 -or $root[0].Name -ne 'mouse-stats.exe') { throw 'ProcessId must identify mouse-stats.exe.' }
$rootCreated = $root[0].CreationDate
if (!(Test-Path -LiteralPath (Join-Path $DataDirectory 'statistics.db') -PathType Leaf)) {
    throw 'statistics.db was not found. Wait for startup or specify the actual -DataDirectory.'
}
$known = @{}
$previous = @{}
$samples = [System.Collections.Generic.List[object]]::new()
$startedAt = [DateTime]::UtcNow
$watch = [System.Diagnostics.Stopwatch]::StartNew()
$initialDbBytes = Get-DatabaseBytes
$cpuSeconds = 0.0
$readBytes = 0.0
$writeBytes = 0.0
$endReason = 'duration_completed'
while ($true) {
    $currentRoot = @($all | Where-Object { $_.ProcessId -eq $ProcessId -and $_.CreationDate -eq $rootCreated })
    if ($currentRoot.Count -ne 1) { $endReason = 'process_exited'; break }
    $known[$ProcessId] = $rootCreated
    # Discover the whole WebView2 subtree, retaining known children across parent exit.
    do {
        $added = $false
        foreach ($p in $all) {
            $id = [int]$p.ProcessId
            $parent = [int]$p.ParentProcessId
            if (!$known.ContainsKey($id) -and $known.ContainsKey($parent) -and $p.CreationDate -ge $known[$parent]) {
                $known[$id] = $p.CreationDate
                $added = $true
            }
        }
    } while ($added)
    $workingBytes = 0.0
    $privateBytes = 0.0
    $count = 0
    $processDetails = [System.Collections.Generic.List[object]]::new()
    foreach ($p in $all) {
        $id = [int]$p.ProcessId
        if (!$known.ContainsKey($id) -or $known[$id] -ne $p.CreationDate) { continue }
        $count++
        $workingBytes += [double]$p.WorkingSetSize
        $privateBytes += [double]$p.PrivatePageCount
        $processDetails.Add([pscustomobject][ordered]@{
            process_id = $id; name = $p.Name
            working_set_bytes = [double]$p.WorkingSetSize
            private_bytes = [double]$p.PrivatePageCount
        })
        $key = "$id/$($p.CreationDate.Ticks)"
        $counters = @((([double]$p.KernelModeTime + [double]$p.UserModeTime) / 10000000.0), [double]$p.ReadTransferCount, [double]$p.WriteTransferCount)
        if ($previous.ContainsKey($key)) {
            # Use a double zero so PowerShell selects the floating-point overload.
            # With integer 0, small CPU deltas can be rounded down to zero.
            $cpuSeconds += [Math]::Max(0.0, [double]$counters[0] - [double]$previous[$key][0])
            $readBytes += [Math]::Max(0, $counters[1] - $previous[$key][1])
            $writeBytes += [Math]::Max(0, $counters[2] - $previous[$key][2])
        }
        $previous[$key] = $counters
    }
    $elapsed = $watch.Elapsed.TotalSeconds
    $samples.Add([pscustomobject][ordered]@{
        elapsed_seconds = $elapsed; processes = $count
        working_set_bytes = $workingBytes; private_bytes = $privateBytes
        observed_cpu_seconds = $cpuSeconds; observed_io_read_bytes = $readBytes
        observed_io_write_bytes = $writeBytes; database_bytes = Get-DatabaseBytes
        process_details = @($processDetails.ToArray())
    })
    if ($elapsed -ge $DurationSeconds) { break }
    Start-Sleep -Milliseconds ([int]([Math]::Min($IntervalSeconds, $DurationSeconds - $elapsed) * 1000))
    $all = Read-Processes
}
$elapsed = $watch.Elapsed.TotalSeconds
$report = [ordered]@{
    started_at = $startedAt.ToString('o'); ended_at = [DateTime]::UtcNow.ToString('o')
    requested_seconds = $DurationSeconds; elapsed_seconds = $elapsed; end_reason = $endReason
    root_process_id = $ProcessId; logical_processors = [Environment]::ProcessorCount
    observed_cpu_seconds = $cpuSeconds
    average_cpu_percent_one_core = 100 * $cpuSeconds / $elapsed
    average_cpu_percent_machine = 100 * $cpuSeconds / $elapsed / [Environment]::ProcessorCount
    peak_sampled_working_set_bytes = ($samples | Measure-Object working_set_bytes -Maximum).Maximum
    peak_sampled_private_bytes = ($samples | Measure-Object private_bytes -Maximum).Maximum
    observed_io_read_bytes = $readBytes; observed_io_write_bytes = $writeBytes
    database_growth_bytes = (Get-DatabaseBytes) - $initialDbBytes
    limitations = @(
        'Sampled process tree counters; short-lived children and work between last sample and exit can be missed.',
        'CPU and I/O count only differences between observations of the same process; new child initial counters are excluded.',
        'Working sets may count shared pages more than once. I/O bytes are not physical disk or network bytes.',
        'Does not verify physical input, polling rate, event loss, consent, recording state, or window visibility.'
    )
    samples = @($samples.ToArray())
}
$absoluteOutput = [IO.Path]::GetFullPath($OutputPath)
$null = New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($absoluteOutput)) -Force
$report | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $absoluteOutput -Encoding UTF8
Write-Output $absoluteOutput
if ($endReason -ne 'duration_completed') { exit 2 }

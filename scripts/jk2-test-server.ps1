[CmdletBinding()]
param(
    [ValidateSet('1.02', '1.04')][string]$Version = '1.04',
    [string]$GameData,
    [ValidateRange(1024, 65535)][int]$Port = 28079,
    [ValidateSet(0, 1)][int]$Pure = 1,
    [ValidatePattern('^[A-Za-z0-9_/-]+$')][string]$Map = 'ffa_bespin',
    [switch]$Stop
)
$ErrorActionPreference = 'Stop'
$project = Split-Path -Parent $PSScriptRoot
$runtime = Join-Path $project 'target/release'
$artifacts = Join-Path $project 'artifacts'
$statePath = Join-Path $artifacts 'jk2-test-server.json'

# Only manage the exact process this helper started, including its creation time.
$existing = $null
$process = $null
if (Test-Path -LiteralPath $statePath) {
    $existing = Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json
    $candidate = Get-Process -Id $existing.ProcessId -ErrorAction SilentlyContinue
    if ($candidate -and $candidate.ProcessName -eq 'jk2mvded' -and
        $candidate.StartTime.ToFileTimeUtc().ToString() -eq $existing.StartTime -and
        $candidate.Path -eq $existing.Executable) { $process = $candidate }
}
if ($Stop) {
    if ($process) {
        $udp = [System.Net.Sockets.UdpClient]::new()
        try {
            $packet = [byte[]](255, 255, 255, 255) + [Text.Encoding]::ASCII.GetBytes("rcon $($existing.Rcon) quit")
            [void]$udp.Send($packet, $packet.Length, '127.0.0.1', [int]$existing.Port)
        } finally { $udp.Dispose() }
        $process | Wait-Process -Timeout 5 -ErrorAction SilentlyContinue
        if (-not $process.HasExited) { $process | Stop-Process }
        Write-Host "Stopped private JK2 test server (PID $($process.Id))."
    } else { Write-Host 'No managed JK2 test server is running.' }
    return
}
if ($process) {
    Write-Host "Private JK2 $($existing.Version) server already running. Use -Stop before changing it."
    Write-Host "jk2_connect $($existing.Version) 127.0.0.1:$($existing.Port)"
    return
}
if (-not $GameData) {
    $ini = Join-Path $runtime 'DinurdoJK-paths.ini'
    foreach ($line in Get-Content -LiteralPath $ini) {
        if ($line -match '^\s*fs_jk2path\s*=(.*)$') { $GameData = $Matches[1].Trim() }
    }
}
if (-not $GameData) { throw 'Supply -GameData <JK2 GameData directory> or configure fs_jk2path.' }
$GameData = [IO.Path]::GetFullPath($GameData)
$server = Join-Path $GameData 'jk2mvded.exe'
if (-not (Test-Path -LiteralPath $server -PathType Leaf)) { throw "Missing $server" }
$serverHome = Join-Path $runtime 'jk2'
[void][IO.Directory]::CreateDirectory($serverHome)
[void][IO.Directory]::CreateDirectory($artifacts)
$rcon = [Guid]::NewGuid().ToString('N')
$arguments = @(
    '+set', 'dedicated', '1', '+set', 'net_ip', '127.0.0.1', '+set', 'net_port', "$Port",
    '+set', 'fs_basepath', ('"' + $GameData + '"'), '+set', 'fs_homepath', ('"' + $serverHome + '"'),
    '+set', 'mv_serverversion', $Version, '+set', 'sv_pure', "$Pure", '+set', 'g_gametype', '0',
    '+set', 'g_teamAutoJoin', '0', '+set', 'sv_maxclients', '8', '+set', 'rconpassword', $rcon,
    '+map', $Map, '+addbot', 'Kyle', '4'
)
# JK2MV 1.4.1's CON_Show crashes in WriteConsoleOutputA when it inherits a
# pseudo-console, including Start-Process -WindowStyle Hidden. Give this old
# dedicated executable no console at all and real log-file handles. Python is
# already used by this repository's source generators and validation tools.
$launch = @{ Executable = $server; Arguments = $arguments; Directory = $serverHome;
    Stdout = (Join-Path $artifacts 'jk2-test-server-stdout.log');
    Stderr = (Join-Path $artifacts 'jk2-test-server-stderr.log') } | ConvertTo-Json -Compress
$launcher = @'
import json, subprocess, sys
task = json.load(sys.stdin)
with open(task['Stdout'], 'w') as out, open(task['Stderr'], 'w') as err:
    args = [value.strip(chr(34)) for value in task['Arguments']]
    process = subprocess.Popen([task['Executable'], *args], cwd=task['Directory'],
        stdout=out, stderr=err, creationflags=subprocess.CREATE_NO_WINDOW)
print(process.pid)
'@
$serverProcessId = $launch | python -c $launcher
if ($LASTEXITCODE -ne 0) { throw 'Could not launch JK2MV (Python 3 is required).' }
$process = Get-Process -Id ([int]$serverProcessId)
@{ ProcessId = $process.Id; StartTime = $process.StartTime.ToFileTimeUtc().ToString();
   Executable = $server; Version = $Version; Port = $Port; Pure = $Pure; Rcon = $rcon } |
    ConvertTo-Json | Set-Content -LiteralPath $statePath
$udp = [System.Net.Sockets.UdpClient]::new()
$udp.Client.ReceiveTimeout = 300
$ready = $false
try {
    $query = [byte[]](255, 255, 255, 255) + [Text.Encoding]::ASCII.GetBytes('getinfo djkready')
    $endpoint = [System.Net.IPEndPoint]::new([System.Net.IPAddress]::Any, 0)
    for ($attempt = 0; $attempt -lt 30; $attempt++) {
        if ($process.HasExited) { throw 'JK2MV exited during startup; see artifacts/jk2-test-server-stderr.log.' }
        [void]$udp.Send($query, $query.Length, '127.0.0.1', $Port)
        try {
            $reply = [Text.Encoding]::ASCII.GetString($udp.Receive([ref]$endpoint))
            $expected = if ($Version -eq '1.02') { '\protocol\15\' } else { '\protocol\16\' }
            if ($reply.Contains($expected)) { $ready = $true; break }
        } catch [System.Net.Sockets.SocketException] { }
        Start-Sleep -Milliseconds 100
    }
} finally { $udp.Dispose() }
if (-not $ready) { throw 'JK2MV did not answer with the requested protocol; inspect its logs or use -Stop.' }
Write-Host "Started private JK2 $Version server (PID $($process.Id))."
Write-Host "jk2_connect $Version 127.0.0.1:$Port"
Write-Host 'Stop with: .\scripts\jk2-test-server.ps1 -Stop'

[CmdletBinding()]
param(
    [string]$BasePath = 'D:\Games\JKA\GameData\base',
    [string]$Map = 'mp/ffa3'
)
# Development-only extraction. This is not the runtime PK3/pure-server filesystem.
$ErrorActionPreference = 'Stop'
if ($Map -notmatch '^[a-zA-Z0-9_-]+(/[a-zA-Z0-9_-]+)*$') {
    throw 'Map must be a relative map name such as mp/ffa3, without an extension.'
}
Add-Type -AssemblyName System.IO.Compression.FileSystem
$projectRoot = Split-Path -Parent $PSScriptRoot
$outputRoot = Join-Path $projectRoot 'target\dev-assets'
$entryName = "maps/$Map.bsp"
$outputFile = Join-Path $outputRoot $entryName
$temporaryFile = "$outputFile.partial"
$found = $false
# Explicit stock-only order: newer numbered stock packages override earlier ones.
foreach ($number in 3..0) {
    $packagePath = Join-Path $BasePath "assets$number.pk3"
    if (-not (Test-Path -LiteralPath $packagePath)) { continue }
    $archive = [IO.Compression.ZipFile]::OpenRead($packagePath)
    try {
        $entries = @($archive.Entries | Where-Object { $_.FullName -ieq $entryName })
        if ($entries.Count -eq 0) { continue }
        if ($entries.Count -ne 1) { throw "Duplicate map entries in $packagePath" }
        $entry = $entries[0]
        if ($entry.Length -lt 152 -or $entry.Length -gt 128MB) { throw 'Map size is outside the importer limit.' }
        $null = New-Item -ItemType Directory -Force -Path (Split-Path -Parent $outputFile)
        $inputStream = $entry.Open()
        try {
            $outputStream = [IO.File]::Create($temporaryFile)
            try {
                $buffer = New-Object byte[] 65536
                [long]$total = 0
                while (($count = $inputStream.Read($buffer, 0, $buffer.Length)) -gt 0) {
                    $total += $count
                    if ($total -gt $entry.Length) { throw 'Expanded map exceeds the declared size.' }
                    $outputStream.Write($buffer, 0, $count)
                }
                if ($total -ne $entry.Length) { throw 'Expanded map is truncated.' }
            } finally { $outputStream.Dispose() }
        } finally { $inputStream.Dispose() }
        Move-Item -LiteralPath $temporaryFile -Destination $outputFile -Force
        [PSCustomObject]@{
            map = $Map
            package = [IO.Path]::GetFullPath($packagePath)
            packageEntry = $entry.FullName
            bytes = $entry.Length
            sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $outputFile).Hash.ToLowerInvariant()
            purpose = 'Local development geometry import; stock numbered packages only; not pure-server verification.'
        } | ConvertTo-Json | Set-Content -Encoding UTF8 -LiteralPath "$outputFile.json"
        $found = $true
        Write-Host "Imported $entryName from $packagePath"
        Write-Host "Map file: $outputFile"
        break
    } finally {
        $archive.Dispose()
        if (Test-Path -LiteralPath $temporaryFile) { Remove-Item -LiteralPath $temporaryFile }
    }
}
if (-not $found) { throw "Map $entryName was not found in stock assets0..3.pk3 under $BasePath" }

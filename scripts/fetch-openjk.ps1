$ErrorActionPreference = 'Stop'
$revision = '1a6a643427aa347553e9073dac5570b33337c4d9'
$projectRoot = Split-Path -Parent $PSScriptRoot
$destination = Join-Path $projectRoot ".references\OpenJK\$revision"
$paths = @(
    'LICENSE.txt',
    'codemp/qcommon/huffman.cpp',
    'codemp/qcommon/msg.cpp',
    'codemp/qcommon/qcommon.h',
    'codemp/qcommon/q_shared.h',
    'codemp/client/cl_parse.cpp'
)
$manifest = foreach ($relativePath in $paths) {
    $destinationFile = Join-Path $destination $relativePath
    $null = New-Item -ItemType Directory -Force -Path (Split-Path -Parent $destinationFile)
    $url = "https://raw.githubusercontent.com/JACoders/OpenJK/$revision/$relativePath"
    Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $destinationFile
    [PSCustomObject]@{
        path = $relativePath
        sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $destinationFile).Hash.ToLowerInvariant()
    }
}
[PSCustomObject]@{
    repository = 'https://github.com/JACoders/OpenJK'
    revision = $revision
    files = @($manifest)
} | ConvertTo-Json -Depth 4 | Set-Content -Encoding UTF8 -LiteralPath (Join-Path $destination 'manifest.json')
Write-Output "Fetched pinned reference sources to $destination"

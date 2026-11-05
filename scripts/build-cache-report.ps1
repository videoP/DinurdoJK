# Non-destructive Cargo build-cache accounting. No cache files are opened or removed.
# "Written" below is the size of new/updated OUTPUT files. It is not a measure
# of physical SSD writes: repeated writes, transient files and OS caching are not
# exposed by Cargo or by filesystem timestamps.

function Get-BuildOutputSnapshot {
    param([string]$BuildDirectory, [string]$RuntimeDirectory)

    $files = [System.Collections.Generic.Dictionary[string, object]]::new([System.StringComparer]::OrdinalIgnoreCase)
    $roots = @($BuildDirectory, $RuntimeDirectory) | Select-Object -Unique
    $incrementalPrefix = (Join-Path $BuildDirectory 'incremental').TrimEnd('\', '/') + [System.IO.Path]::DirectorySeparatorChar
    foreach ($root in $roots) {
        if (-not [System.IO.Directory]::Exists($root)) { continue }
        # Never descend into the runtime base/mod directories: they can hold tens
        # of gigabytes of JKA assets unrelated to Rust compilation.
        $directories = @($root)
        if ($root -eq $BuildDirectory) {
            $directories += @('deps', 'incremental', 'build', '.fingerprint', 'examples') |
                ForEach-Object { Join-Path $root $_ }
        }
        foreach ($directory in $directories) {
            if (-not [System.IO.Directory]::Exists($directory)) { continue }
            $option = if ($directory -eq $root) {
                [System.IO.SearchOption]::TopDirectoryOnly
            } else {
                [System.IO.SearchOption]::AllDirectories
            }
            try {
                foreach ($path in [System.IO.Directory]::EnumerateFiles($directory, '*', $option)) {
                    try {
                        $info = [System.IO.FileInfo]::new($path)
                        $fileId = $null
                        if ($path.StartsWith($incrementalPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
                            $fileId = Get-CacheFileIdentity $path
                        }
                        $files[$info.FullName] = [pscustomobject]@{
                            Bytes = [long]$info.Length
                            Ticks = [long]$info.LastWriteTimeUtc.Ticks
                            FileId = $fileId
                        }
                    } catch [System.IO.IOException] {
                        # A temporary output may disappear during enumeration.
                    }
                }
            } catch [System.IO.IOException] {
                Write-Warning "Could not enumerate all of ${directory}: $($_.Exception.Message)"
            } catch [System.UnauthorizedAccessException] {
                Write-Warning "Access denied while enumerating ${directory}: $($_.Exception.Message)"
            }
        }
    }
    # Only known runtime support files are included, never entire asset trees.
    if ($BuildDirectory -ne $RuntimeDirectory) {
        foreach ($path in @(
            (Join-Path $RuntimeDirectory 'base\dinurdo-assets.pk3'),
            (Join-Path $RuntimeDirectory 'base\shader_modes_ab.cfg'),
            (Join-Path $RuntimeDirectory 'licenses\OpenAL-Soft-LGPL-2.0.txt'),
            (Join-Path $RuntimeDirectory 'licenses\EalMan-LGPL-2.1.txt'),
            (Join-Path $RuntimeDirectory 'licenses\Bevy-Solari-MIT.txt')
        )) {
            if ([System.IO.File]::Exists($path)) {
                $info = [System.IO.FileInfo]::new($path)
                $files[$info.FullName] = [pscustomobject]@{
                    Bytes = [long]$info.Length
                    Ticks = [long]$info.LastWriteTimeUtc.Ticks
                    FileId = $null
                }
            }
        }
    }
    return ,$files
}

function New-CargoArtifactReport {
    return [pscustomobject]@{
        FreshTargets = 0
        BuiltTargets = 0
        FreshFiles = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
        BuiltFiles = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
        AllFiles = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
    }
}

function Add-CargoArtifactMessage {
    param([object]$Report, [object]$Message)
    if ($Message.fresh) { $Report.FreshTargets++ } else { $Report.BuiltTargets++ }
    # Do NOT assign an empty HashSet through a PowerShell `if` expression:
    # pipeline enumeration collapses an empty collection to $null. A subsequent
    # .Add() then crashes the entire build with "null-valued expression".
    if ($Message.fresh) {
        $destination = $Report.FreshFiles
    } else {
        $destination = $Report.BuiltFiles
    }
    foreach ($path in @($Message.filenames) + @($Message.executable)) {
        if ([string]::IsNullOrWhiteSpace($path)) { continue }
        # Cargo reports absolute artifact paths; normalize for case-insensitive lookup.
        $full = [System.IO.Path]::GetFullPath([string]$path)
        [void]$destination.Add($full)
        [void]$Report.AllFiles.Add($full)
    }
}

function Format-BuildBytes([long]$Value) {
    if ($Value -ge 1GB) { return ('{0:N2} GiB' -f ($Value / 1GB)) }
    if ($Value -ge 1MB) { return ('{0:N1} MiB' -f ($Value / 1MB)) }
    if ($Value -ge 1KB) { return ('{0:N1} KiB' -f ($Value / 1KB)) }
    return "$Value B"
}

function Get-BuildOutputBucket {
    param([string]$Path, [string]$BuildDirectory, [string]$RuntimeDirectory)
    $prefix = $BuildDirectory.TrimEnd('\', '/') + [System.IO.Path]::DirectorySeparatorChar
    if ($Path.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        $relative = $Path.Substring($prefix.Length)
        $component = ($relative -split '[\\/]')[0]
        if ($component -in @('deps', 'incremental', 'build', '.fingerprint', 'examples')) {
            return "target\$(Split-Path -Leaf $BuildDirectory)\$component"
        }
        return "target\$(Split-Path -Leaf $BuildDirectory)\(root)"
    }
    return "target\$(Split-Path -Leaf $RuntimeDirectory)\(runtime files)"
}

# Compact, color-coded output. Location labels are relative to target\<profile>
# except entries explicitly marked target\release (staged runtime files).
function Write-CacheLocationSummary {
    param([System.Collections.IDictionary]$Buckets, [string]$BuildDirectory, [ConsoleColor]$Color = [ConsoleColor]::DarkGray)
    if ($Buckets.Count -eq 0) { return }
    $prefix = "target\$(Split-Path -Leaf $BuildDirectory)\"
    $parts = [System.Collections.Generic.List[string]]::new()
    foreach ($entry in ($Buckets.GetEnumerator() | Sort-Object Value -Descending)) {
        $name = [string]$entry.Key
        if ($name.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
            $name = $name.Substring($prefix.Length)
        }
        $parts.Add("$name $(Format-BuildBytes ([long]$entry.Value))")
    }
    Write-Host "    $($parts -join ' | ')" -ForegroundColor $Color
}

function Show-BuildCacheReport {
    param(
        [string]$BuildDirectory,
        [string]$RuntimeDirectory,
        [object]$Before,
        [object]$Artifacts
    )
    $after = Get-BuildOutputSnapshot -BuildDirectory $BuildDirectory -RuntimeDirectory $RuntimeDirectory
    $changed = @{}
    $reused = @{}
    $untouched = @{}
    $newBytes = [long]0
    $updatedBytes = [long]0
    $deletedBytes = [long]0
    $untouchedBytes = [long]0
    $incrementalBytes = [long]0
    $reusedBytes = [long]0

    foreach ($path in $after.Keys) {
        $info = $after[$path]
        $bucket = Get-BuildOutputBucket -Path $path -BuildDirectory $BuildDirectory -RuntimeDirectory $RuntimeDirectory
        if (-not $Before.ContainsKey($path)) {
            $newBytes += $info.Bytes
            if (-not $changed.ContainsKey($bucket)) { $changed[$bucket] = [long]0 }
            $changed[$bucket] += $info.Bytes
        } elseif ($Before[$path].Ticks -ne $info.Ticks -or $Before[$path].Bytes -ne $info.Bytes) {
            $updatedBytes += $info.Bytes
            if (-not $changed.ContainsKey($bucket)) { $changed[$bucket] = [long]0 }
            $changed[$bucket] += $info.Bytes
        }
        if ($Artifacts.FreshFiles.Contains($path)) {
            $reusedBytes += $info.Bytes
            if (-not $reused.ContainsKey($bucket)) { $reused[$bucket] = [long]0 }
            $reused[$bucket] += $info.Bytes
        }
        # Unchanged file metadata does NOT prove the file was not read: Cargo
        # and rustc routinely reuse artifacts without updating their timestamps.
        if ($bucket -match '\\(deps|incremental|build|\.fingerprint)$' -and
            $Before.ContainsKey($path) -and
            $Before[$path].Ticks -eq $info.Ticks -and
            $Before[$path].Bytes -eq $info.Bytes) {
            $untouchedBytes += $info.Bytes
            if (-not $untouched.ContainsKey($bucket)) { $untouched[$bucket] = [long]0 }
            $untouched[$bucket] += $info.Bytes
        }
        if ($bucket -match '\\incremental$') { $incrementalBytes += $info.Bytes }
    }
    foreach ($path in $Before.Keys) {
        if (-not $after.ContainsKey($path)) { $deletedBytes += $Before[$path].Bytes }
    }

    # A new rustc session can hard-link gigabytes of unchanged work products.
    # File lengths summed by path count that data again; NTFS file identities do
    # not. Compare identities across snapshots even if old names were deleted.
    # These are final file sizes, never a claim of physical SSD write counters.
    $oldCacheIds = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
    foreach ($old in $Before.Values) {
        if ($old.FileId) { [void]$oldCacheIds.Add([string]$old.FileId) }
    }
    $currentCacheIds = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
    $newCacheIds = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
    $cacheUniqueBytes = [long]0
    $cacheNewIdentityBytes = [long]0
    $cacheReusedLinkBytes = [long]0
    $unidentifiedCacheBytes = [long]0
    $unidentifiedCacheFiles = 0
    foreach ($path in $after.Keys) {
        $item = $after[$path]
        $bucket = Get-BuildOutputBucket -Path $path -BuildDirectory $BuildDirectory -RuntimeDirectory $RuntimeDirectory
        if ($bucket -notmatch '\\incremental$') { continue }
        if (-not $item.FileId) {
            $unidentifiedCacheBytes += $item.Bytes
            $unidentifiedCacheFiles++
            continue
        }
        if ($currentCacheIds.Add([string]$item.FileId)) {
            $cacheUniqueBytes += $item.Bytes
        }
        if (-not $oldCacheIds.Contains([string]$item.FileId)) {
            if ($newCacheIds.Add([string]$item.FileId)) {
                $cacheNewIdentityBytes += $item.Bytes
            }
        } elseif (-not $Before.ContainsKey($path)) {
            $cacheReusedLinkBytes += $item.Bytes
        }
    }

    Write-Host "`n[JKA CACHE] target\$(Split-Path -Leaf $BuildDirectory) | Cargo: $($Artifacts.FreshTargets) fresh, $($Artifacts.BuiltTargets) compiled" -ForegroundColor Cyan

    Write-Host '  Reused artifacts: ' -NoNewline
    Write-Host (Format-BuildBytes $reusedBytes) -ForegroundColor Green -NoNewline
    Write-Host ' (Cargo fresh; on-disk sizes)' -ForegroundColor DarkGray
    Write-CacheLocationSummary -Buckets $reused -BuildDirectory $BuildDirectory -Color DarkGreen

    Write-Host '  Changed paths:    ' -NoNewline
    Write-Host (Format-BuildBytes ($newBytes + $updatedBytes)) -ForegroundColor Yellow -NoNewline
    Write-Host " (new $(Format-BuildBytes $newBytes), modified $(Format-BuildBytes $updatedBytes))" -ForegroundColor DarkGray
    Write-CacheLocationSummary -Buckets $changed -BuildDirectory $BuildDirectory -Color DarkYellow
    if ($deletedBytes -gt 0) {
        Write-Host "  Removed:          $(Format-BuildBytes $deletedBytes) (previous file sizes)" -ForegroundColor DarkGray
    }

    Write-Host '  Unchanged cache:  ' -NoNewline
    Write-Host (Format-BuildBytes $untouchedBytes) -ForegroundColor Gray -NoNewline
    Write-Host ' (not necessarily unused)' -ForegroundColor DarkGray
    Write-CacheLocationSummary -Buckets $untouched -BuildDirectory $BuildDirectory

    if ($script:JkaNtfsCacheIdReady -and $unidentifiedCacheFiles -eq 0) {
        $incrementalLabel = if ($env:CARGO_INCREMENTAL -eq '0') { '  Old incremental:  ' } else { '  Incremental:      ' }
        Write-Host $incrementalLabel -NoNewline
        $cacheColor = if ($cacheUniqueBytes -gt 20GB) { 'Yellow' } else { 'Cyan' }
        Write-Host "$(Format-BuildBytes $cacheUniqueBytes) unique / $(Format-BuildBytes $incrementalBytes) by path" -ForegroundColor $cacheColor
        Write-Host "    New file IDs: $(Format-BuildBytes $cacheNewIdentityBytes) | new hard-link names: $(Format-BuildBytes $cacheReusedLinkBytes)" -ForegroundColor Cyan
        if ($incrementalBytes -gt $cacheUniqueBytes) {
            Write-Host "    Shared cache data: $(Format-BuildBytes ($incrementalBytes - $cacheUniqueBytes)) (duplicate names, not duplicated data)" -ForegroundColor Green
        }
    } else {
        $incrementalLabel = if ($env:CARGO_INCREMENTAL -eq '0') { 'Old incremental' } else { 'Incremental' }
        Write-Host "  ${incrementalLabel}: $(Format-BuildBytes $incrementalBytes) by path (NTFS de-dup unavailable)" -ForegroundColor Cyan
    }
    if ($env:CARGO_INCREMENTAL -eq '0' -and $incrementalBytes -gt 0) {
        Write-Host '    Existing incremental files are retained; not used by this build.' -ForegroundColor DarkGray
    }
    Write-Host '  Unreferenced:     unknown; no automatic deletion.' -ForegroundColor DarkGray
    Write-Host '  Sizes are final file lengths, not actual SSD writes. New hard links write metadata only.' -ForegroundColor DarkGray
}

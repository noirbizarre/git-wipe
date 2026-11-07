$ErrorActionPreference = 'Stop'

# Run again after cache restoration: extraction must not replace the junction with an NTFS directory.
$target = Get-Item "$env:GITHUB_WORKSPACE/target"
$expected = [System.IO.Path]::GetFullPath("$env:DEV_DRIVE/target")
if ($target.LinkType -ne 'Junction' -or $target.Target -ne $expected) {
    throw 'The Cargo target junction changed; check TAR_OPTIONS and the cache restore before retrying.'
}

# Resolve files on both destinations, not the backing VHDX: a missing mount could leave NTFS directories behind.
foreach ($directory in @($expected, "$env:DEV_DRIVE/test-temp")) {
    if (-not (Test-Path -LiteralPath $directory -PathType Container)) {
        throw "Storage directory '$directory' is missing; check the Windows Dev Drive setup before retrying."
    }
    $probe = New-Item -ItemType File -Path "$directory/.storage-check-$([guid]::NewGuid())"
    try {
        $volume = Get-Volume -FilePath $probe.FullName
        if ($volume.FileSystem -ne 'ReFS') {
            throw "Storage directory '$directory' is not on ReFS; check the Windows runner image and Dev Drive setup before retrying."
        }
    } finally {
        Remove-Item $probe.FullName
    }
}

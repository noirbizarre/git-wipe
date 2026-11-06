$ErrorActionPreference = 'Stop'

# Run again after cache restoration: extraction must not replace the junction with an NTFS directory.
$target = Get-Item "$env:GITHUB_WORKSPACE/target"
$expected = [System.IO.Path]::GetFullPath("$env:DEV_DRIVE/target")
if ($target.LinkType -ne 'Junction' -or $target.Target -ne $expected) {
    throw 'The Cargo target junction changed; check TAR_OPTIONS and the cache restore before retrying.'
}

# Resolve a file on the destination, not the backing VHDX: a missing mount could leave an NTFS directory behind.
$probe = New-Item -ItemType File -Path "$expected/.storage-check-$([guid]::NewGuid())"
try {
    $volume = Get-Volume -FilePath $probe.FullName
    if ($volume.FileSystem -ne 'ReFS') {
        throw 'The build volume is not ReFS; check the Windows runner image and Dev Drive setup before retrying.'
    }
} finally {
    Remove-Item $probe.FullName
}

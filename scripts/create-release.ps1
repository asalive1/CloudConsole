. "$PSScriptRoot\project-settings.ps1"

Clear-Host

Write-Host ""
Write-Host "================================="
Write-Host " Create Release"
Write-Host "================================="
Write-Host ""

$version = Read-Host "Release Version"

if ([string]::IsNullOrWhiteSpace($version)) {
    Write-Host ""
    Write-Host "Release cancelled."
    Write-Host ""
    exit
}

$tagName = "$VersionPrefix$version"

Write-Host ""
Write-Host "Preparing Release:"
Write-Host "  $tagName"
Write-Host ""

$existingTag = git tag -l $tagName

if ($existingTag) {
    Write-Host ""
    Write-Host "ERROR: Release already exists."
    Write-Host "Tag: $tagName"
    Write-Host ""
    exit
}

git tag $tagName

if ($LASTEXITCODE -ne 0) {
    Write-Host ""
    Write-Host "ERROR: Failed to create tag."
    Write-Host ""
    exit
}

git push origin $tagName

if ($LASTEXITCODE -ne 0) {
    Write-Host ""
    Write-Host "ERROR: Failed to push tag."
    Write-Host ""
    exit
}

Write-Host ""
Write-Host "================================="
Write-Host " Release Created Successfully"
Write-Host "================================="
Write-Host ""
Write-Host "Tag:"
Write-Host "  $tagName"
Write-Host ""
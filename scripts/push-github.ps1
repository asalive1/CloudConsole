. "$PSScriptRoot\project-settings.ps1"

Clear-Host

Write-Host ""
Write-Host "================================="
Write-Host " GitHub Backup"
Write-Host "================================="
Write-Host ""

$status = git status -sb

if ($status -notmatch "ahead") {
    Write-Host "Repository already synced."
    Write-Host ""
    exit
}

git push origin main

if ($LASTEXITCODE -ne 0) {
    Write-Host ""
    Write-Host "GitHub push failed."
    Write-Host ""
    exit
}

git push --tags

Write-Host ""
Write-Host "================================="
Write-Host " Backup Complete"
Write-Host "================================="
Write-Host ""
. "$PSScriptRoot\\project-settings.ps1"

Write-Host ""
Write-Host "================================="
Write-Host " Project Status"
Write-Host "================================="
Write-Host ""

Write-Host "Project:"
Write-Host "  $ProjectName"
Write-Host ""

Write-Host "Branch:"
git branch --show-current

Write-Host ""
Write-Host "Modified Files:"
git status --short

Write-Host ""
Write-Host "Latest Commit:"
git log --oneline -1

Write-Host ""
Write-Host "================================="
Write-Host ""
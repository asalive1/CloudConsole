. "$PSScriptRoot\project-settings.ps1"

Clear-Host

Write-Host ""
Write-Host "================================="
Write-Host " Project Status"
Write-Host "================================="
Write-Host ""

Write-Host "Project:"
Write-Host "  $ProjectName"
Write-Host ""

$status = git status -sb

Write-Host "Repository State:"

if ($status -match "ahead") {
    Write-Host "  Backup Needed"
}
elseif ($status -match "behind") {
    Write-Host "  Local Repository Behind GitHub"
}
elseif ($status -match "M " -or $status -match "\?\?") {
    Write-Host "  Uncommitted Changes Present"
}
else {
    Write-Host "  Synced With GitHub"
}

Write-Host ""

Write-Host "Current Branch:"
Write-Host "  $(git branch --show-current)"
Write-Host ""

Write-Host "Repository Status:"
Write-Host "$status"

Write-Host ""

Write-Host "Last Commit:"
git log --oneline -1

Write-Host ""

Write-Host "Recent History:"
git log --oneline -5

Write-Host ""

$tags = git tag --sort=-version:refname | Select-Object -First 5

Write-Host "Recent Releases:"

if ($tags) {
    $tags
}
else {
    Write-Host "  No releases created yet."
}

Write-Host ""
Write-Host "================================="
Write-Host ""
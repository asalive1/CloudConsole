. "$PSScriptRoot\project-settings.ps1"

Clear-Host

Write-Host ""
Write-Host "================================="
Write-Host " Build / Deploy"
Write-Host "================================="
Write-Host ""

if ([string]::IsNullOrWhiteSpace($DevBuildCommand)) {
    Write-Host "No build command configured."
    Write-Host ""
    Write-Host "Update project-settings.ps1 to enable builds."
    Write-Host ""
    exit
}

Write-Host "Executing:"
Write-Host "  $DevBuildCommand"
Write-Host ""

Invoke-Expression $DevBuildCommand

if ($LASTEXITCODE -eq 0) {
    Write-Host ""
    Write-Host "================================="
    Write-Host " Build Complete"
    Write-Host "================================="
    Write-Host ""
}
else {
    Write-Host ""
    Write-Host "================================="
    Write-Host " Build Failed"
    Write-Host "================================="
    Write-Host ""
}
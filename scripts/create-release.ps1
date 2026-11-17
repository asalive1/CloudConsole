$version = Read-Host "Release Version"

git tag "v$version"

git push origin "v$version"

Write-Host ""
Write-Host "Release Created"
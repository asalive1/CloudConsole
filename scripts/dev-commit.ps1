$message = Read-Host "Commit Message"

git add .

git commit -m "$message"

Write-Host ""
Write-Host "Checkpoint Saved"
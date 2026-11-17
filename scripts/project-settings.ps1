#################################################
# Project Settings
#################################################

$ProjectName = "CloudConsole"

$GitBranch = "main"

$VersionPrefix = "v"

#
# Build Commands
#

$DevBuildCommand = "docker compose -f docker-compose.local.yml up -d"

$ProdBuildCommand = "docker compose -f docker-compose.yml up -d"
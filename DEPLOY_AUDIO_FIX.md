# Cloud Console Audio Fix - Deployment Guide

## Issue
Your Cloud Console is experiencing **choppy/stuttering audio** on the Loopback output due to an invalid RTP payload type configuration.

## Quick Fix

To apply the fix and restore audio quality:

### On the deployment machine (Linux):

\\\ash
# Navigate to CloudConsole directory
cd /path/to/CloudConsole

# Pull the latest changes
git pull

# Restart the services (this applies the database migration and rebuilds backend)
./deploy/local/station.sh s2 up

# Monitor the logs to confirm successful startup
./deploy/local/station.sh s2 logs
\\\

### Expected Outcome

After running these commands:
1. ✅ Database migration will fix existing invalid payload_type values
2. ✅ Backend will rebuild with new validation
3. ✅ All services will restart with the fix applied
4. ✅ Loopback audio output will work correctly
5. ✅ No more choppy/stuttering audio

## What Was Fixed

**Problem**: An RTP output had a payload_type value > 127, which violates the RTP standard (valid range: 0-127)

**Root Cause**: This caused the audio engine to reject the configuration, leading to buffer drops and choppy audio

**Solution**: 
- Database migration resets invalid payload_type to 100 (a valid dynamic RTP type)
- Backend now validates payload_type on input, preventing future issues
- Audio engine validates on output (defense-in-depth)

## Verification

After deployment, verify the fix by:

1. Checking that Loopback output shows in the mixer without errors
2. Listening to the audio - it should no longer be choppy
3. Checking logs for successful configuration:
   \\\ash
   ./deploy/local/station.sh s2 logs | grep -i "loopback\|payload"
   \\\
   Should NOT show any "payload_type" errors

## Troubleshooting

If audio is still choppy after applying the fix:

1. Verify the migration ran successfully:
   \\\ash
   ./deploy/local/station.sh s2 exec postgres psql -U cloud_console -d cloud_console -c "\\
   SELECT id, name, config->>'payload_type' FROM outputs WHERE output_type='rtp';"
   \\\
   All payload_type values should be ≤ 127

2. Check for other configuration issues:
   \\\ash
   ./deploy/local/station.sh s2 logs backend | tail -50
   \\\

3. Restart services individually if needed:
   \\\ash
   ./deploy/local/station.sh s2 restart backend audio-engine
   \\\

## Technical Details

See \AUDIO_FIX_SUMMARY.md\ for detailed technical information about:
- Root cause analysis
- Code changes made
- RTP payload type specifications
- Prevention mechanisms

---
**Status**: ✅ Ready for deployment
**Latest Commit**: 5729f53 Fix choppy audio caused by invalid RTP payload_type
**Files Modified**: 3 (1 migration, 1 backend change, 1 summary doc)

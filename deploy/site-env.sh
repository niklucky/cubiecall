# Loaded from the repository root by deployment and release commands.
site_env=${CUBIE_SITE_ENV:-deploy/site.env.local}
if [ -f "$site_env" ]; then
    set -a
    . "$site_env"
    set +a
fi

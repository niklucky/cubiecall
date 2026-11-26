#!/usr/bin/env python3
"""Render a deployment's public settings, nginx sites, and download page."""
import argparse
import ipaddress
import os
from pathlib import Path
import re
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parent
LABEL = r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?"
DOMAIN = re.compile(rf"{LABEL}(?:\.{LABEL})+")


def host(value, key):
    if len(value) > 253 or not DOMAIN.fullmatch(value.lower()):
        raise ValueError(f"{key} must be a DNS hostname")
    return value.lower()


def origin(value, key, optional=False):
    if optional and not value:
        return "", ""
    url = urlsplit(value)
    if url.scheme != "https" or url.path not in ("", "/") or value != value.strip():
        raise ValueError(f"{key} must be an HTTPS origin")
    name = host(url.netloc, key)
    if value.rstrip("/") != f"https://{url.netloc}":
        raise ValueError(f"{key} must be an HTTPS origin without query or credentials")
    return f"https://{name}", name


def substitute(template, values):
    return re.sub(r"@([A-Z_]+)@", lambda match: values[match[1]], template)


def render_downloads(output, public, reserve, team):
    if team and not re.fullmatch(r"[A-Z0-9]{10}", team):
        raise ValueError("CUBIE_MAC_TEAM_ID must be an Apple team ID")
    terminal = ""
    if team:
        terminal = (
            '  <h2>Install on a Mac using Terminal</h2>\n'
            f"  <pre>curl -fsSL --proto-redir '=https' {public}/download/install.sh | sh</pre>"
        )
        if reserve:
            terminal += (
                '\n  <p class="dim">If the primary is unavailable, use the reserve domain.</p>\n'
                f"  <pre>curl -fsSL --proto-redir '=https' {reserve}/download/install.sh | sh -s -- --reserve</pre>"
            )
        terminal += '\n  <p class="dim">Puts Cubie in Applications and opens it. Cubie checks for updates automatically.</p>'

    values = {
        "PUBLIC_URL": public,
        "RESERVE_URL": reserve,
        "MAC_TEAM": team,
        "TERMINAL_INSTALL": terminal,
    }
    output.mkdir(parents=True, exist_ok=True)
    for name in ["index.html", "install.sh"]:
        target = output / name
        if name == "install.sh" and not team:
            target.unlink(missing_ok=True)
        else:
            target.write_text(substitute((ROOT / "download" / name).read_text(), values))


def render(output, env, downloads_only):
    public, primary = origin(env.get("CUBIE_PUBLIC_URL", ""), "CUBIE_PUBLIC_URL")
    reserve, backup = origin(env.get("CUBIE_RESERVE_URL", ""), "CUBIE_RESERVE_URL", True)
    team = env.get("CUBIE_MAC_TEAM_ID", "")
    if downloads_only:
        render_downloads(output / "download", public, reserve, team)
        return
    legacy, old = origin(env.get("CUBIE_LEGACY_URL", ""), "CUBIE_LEGACY_URL", True)
    redirects = [host(name, "CUBIE_REDIRECT_HOSTS") for name in env.get("CUBIE_REDIRECT_HOSTS", "").split()]
    apex = [primary, *redirects, *([old] if old else [])]
    www = [f"www.{name}" for name in apex]
    all_hosts = apex + www + ([backup] if backup else [])
    if len(all_hosts) != len(set(all_hosts)):
        raise ValueError("Public, reserve, and alias hostnames must be distinct")

    peer = env.get("CUBIE_RESERVE_PROXY_IP", "")
    upstream = env.get("CUBIE_RESERVE_UPSTREAM_IP", "")
    if bool(reserve) != bool(peer) or bool(reserve) != bool(upstream):
        raise ValueError("Set CUBIE_RESERVE_URL, CUBIE_RESERVE_PROXY_IP and CUBIE_RESERVE_UPSTREAM_IP together")
    if reserve:
        peer = str(ipaddress.IPv4Address(peer))
        upstream = str(ipaddress.IPv4Address(upstream))
    email = env.get("CUBIE_ACME_EMAIL", "")
    if email and not re.fullmatch(r"[A-Za-z0-9_.+%=-]+@[A-Za-z0-9.-]+", email):
        raise ValueError("CUBIE_ACME_EMAIL must be an email address")
    values = {
        "PRIMARY_HOST": primary,
        "RESERVE_HOST": backup,
        "UPSTREAM_IP": upstream,
        "ORIGIN_HOSTS": " ".join(apex),
        "WWW_HOSTS": " ".join(www),
        "CERTIFICATE_HOSTS": " ".join(apex + www),
        "LEGACY_HTTP_MAP": f"    {old} {old};\n    www.{old} {old};" if old else "",
        "LEGACY_FRAGMENT_MAP": f'    "{old}::/" 1;' if old else "",
        "LEGACY_GUEST_MAP": f'    "{old}:" 1;' if old else "",
        "RESERVE_PEER_MAP": f"    {peer} {backup};" if reserve else "",
        "RESERVE_REAL_IP": f"        set_real_ip_from {peer};\n        real_ip_header X-Real-IP;" if reserve else "",
        "CANONICAL_REDIRECTS": "\n".join(
            f"        if ($host = {alias}) {{ return 308 {public}$request_uri; }}" for alias in redirects
        ),
    }

    render_downloads(output / "download", public, reserve, team)
    for site in ["origin.conf", "origin.acme.conf", "origin-upgrade.acme.conf", "reserve.conf", "reserve.acme.conf"]:
        target = output / site
        if site.startswith("reserve.") and not reserve:
            target.unlink(missing_ok=True)
            continue
        template = "origin.acme.conf" if site == "origin-upgrade.acme.conf" else site
        acme_hosts = apex + www
        if site == "origin-upgrade.acme.conf" and old:
            acme_hosts = [name for name in acme_hosts if name not in (old, f"www.{old}")]
        (output / site).write_text(substitute(
            (ROOT / "nginx" / f"{template}.template").read_text(),
            {**values, "ACME_HOSTS": " ".join(acme_hosts)},
        ))
    (output / "settings.env").write_text("".join(
        f"{key}='{value}'\n" for key, value in {
            "DOMAIN": primary,
            "LEGACY_HOST": old,
            "ORIGIN_NAMES": " ".join(apex),
            "WWW_NAMES": " ".join(www),
            "PUBLIC_URL": public,
            "LEGACY_URL": legacy,
            "ACME_EMAIL": email,
        }.items()
    ))
    (output / "public.env").write_text(
        f"CUBIE_PUBLIC_URL={public}\nCUBIE_LEGACY_URL={legacy}\n"
        f"CUBIE_RESERVE_URL={reserve}\nCUBIE_RESERVE_PROXY_IP={peer}\n"
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--downloads-only", action="store_true")
    args = parser.parse_args()
    try:
        render(args.output, os.environ, args.downloads_only)
    except (ValueError, OSError) as error:
        parser.exit(1, f"Site configuration: {error}\n")

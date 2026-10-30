#!/usr/bin/env bash
# Génère un dépôt pacman signé à partir des paquets Arch construits.
#
# Usage:
#   packaging/arch/build_repository.sh --tag vX.Y.Z --packages-dir DIR --output-dir DIR --private-key FILE
#
# Arbre de sortie :
#   output/
#     x86_64/
#       ltbox.db.tar.gz          (base de données signée)
#       ltbox.db.tar.gz.sig
#       ltbox-<version>-1-x86_64.pkg.tar.zst
#     aarch64/
#       ...
#     index.html                 (instructions d'installation)
#     ltbox-repo.asc             (clé publique du dépôt)

set -euo pipefail

tag=""
packages_dir=""
output_dir=""
private_key=""

usage() {
    echo "Usage: $0 --tag vX.Y.Z --packages-dir DIR --output-dir DIR --private-key FILE" >&2
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --tag) tag="$2"; shift 2 ;;
        --packages-dir) packages_dir="$2"; shift 2 ;;
        --output-dir) output_dir="$2"; shift 2 ;;
        --private-key) private_key="$2"; shift 2 ;;
        *) usage; exit 2 ;;
    esac
done

if [[ -z "${tag}" || -z "${packages_dir}" || -z "${output_dir}" || -z "${private_key}" ]]; then
    usage
    exit 2
fi
if [[ ! "${tag}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([+-][0-9A-Za-z.-]+)?$ ]]; then
    echo "Release tag is not v-prefixed SemVer: ${tag}" >&2
    exit 2
fi
if [[ -z "${REPO_GPG_PASSPHRASE:-}" ]]; then
    echo "REPO_GPG_PASSPHRASE is required." >&2
    exit 2
fi

packages_dir="$(realpath "${packages_dir}")"
output_dir="$(realpath -m "${output_dir}")"
private_key="$(realpath "${private_key}")"
version="${tag#v}"

rm -rf "${output_dir}"
mkdir -p "${output_dir}/x86_64" "${output_dir}/aarch64"

declare -A pkg_by_arch=(
    [x86_64]="ltbox-${version}-1-x86_64.pkg.tar.zst"
    [aarch64]="ltbox-${version}-1-aarch64.pkg.tar.zst"
)

for arch in x86_64 aarch64; do
    package="${pkg_by_arch[${arch}]}"
    if [[ ! -f "${packages_dir}/${package}" ]]; then
        echo "Missing Arch package: ${packages_dir}/${package}" >&2
        exit 1
    fi
    if [[ ! -f "${packages_dir}/${package}.sha256" ]]; then
        echo "Missing package checksum: ${packages_dir}/${package}.sha256" >&2
        exit 1
    fi
    (cd "${packages_dir}" && sha256sum --check "${package}.sha256")
    install -m 0644 "${packages_dir}/${package}" "${output_dir}/${arch}/${package}"
done

# --- Base de données pacman par arch ---
for arch in x86_64 aarch64; do
    package="${pkg_by_arch[${arch}]}"
    (
        cd "${output_dir}/${arch}"
        repo-add --quiet ltbox.db.tar.gz "${package}"
        # Supprime les vieux fichiers .db et .files non compressés laissés par repo-add.
        rm -f ltbox.db ltbox.files
    )
done

# --- Signature du dépôt ---
gnupg_home="$(mktemp -d)"
cleanup() { rm -rf "${gnupg_home}"; }
trap cleanup EXIT
chmod 0700 "${gnupg_home}"
export GNUPGHOME="${gnupg_home}"

gpg --batch --quiet --import "${private_key}"
signing_fingerprint="$(
    gpg --batch --with-colons --list-secret-keys |
        awk -F: '
            ($1 == "sec" || $1 == "ssb") && tolower($12) ~ /s/ {
                want_fingerprint = 1
                next
            }
            want_fingerprint && $1 == "fpr" {
                print $10
                exit
            }
        '
)"
if [[ -z "${signing_fingerprint}" ]]; then
    echo "The repository private key has no signing-capable key." >&2
    exit 1
fi

gpg --batch --armor --export "${signing_fingerprint}" \
    > "${output_dir}/ltbox-repo.asc"

gpg_sign() {
    gpg --batch --yes --quiet \
        --pinentry-mode loopback \
        --passphrase "${REPO_GPG_PASSPHRASE}" \
        --local-user "${signing_fingerprint}" \
        --digest-algo SHA256 \
        "$@"
}

for arch in x86_64 aarch64; do
    gpg_sign --armor --detach-sign \
        --output "${output_dir}/${arch}/ltbox.db.tar.gz.sig" \
        "${output_dir}/${arch}/ltbox.db.tar.gz"
done

# --- index.html (GitHub Pages) ---
cat > "${output_dir}/index.html" <<HTML
<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>LTBox Arch Linux repository</title>
</head>
<body>
  <main>
    <h1>LTBox Arch Linux repository</h1>
    <p>This repository contains only the current LTBox release, <strong>${tag}</strong>.
    Older packages remain available on the <a href="https://github.com/miner7222/LTBox/releases">GitHub Releases page</a>.</p>
    <p>Import the repository signing key, then add to <code>/etc/pacman.conf</code>:</p>
    <pre><code>[ltbox]
SigLevel = Required DatabaseOptional
Server = https://miner7222.github.io/ltbox-arch-repo/\$arch</code></pre>
    <p>The public key is available at the stable <a href="ltbox-repo.asc">ltbox-repo.asc</a> URL.
    Its OpenPGP fingerprint is <code>${signing_fingerprint}</code>.</p>
    <p>After importing the key (<code>pacman-key --add ltbox-repo.asc</code> then
    <code>pacman-key --lsign-key &lt;fingerprint&gt;</code>), install with:</p>
    <pre><code>sudo pacman -Sy ltbox</code></pre>
  </main>
</body>
</html>
HTML

touch "${output_dir}/.nojekyll"
echo "Built signed Arch Linux repository for ${tag} in ${output_dir}."

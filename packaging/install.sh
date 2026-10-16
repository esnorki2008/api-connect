#!/bin/sh
# Instala api-connect para el usuario actual, sin sudo: binarios en ~/.local/bin y acceso en el menú de aplicaciones.
#   ./install.sh               instala (desde el paquete descargado, o compila si se corre desde el repo)
#   ./install.sh --uninstall   desinstala (tus datos en ~/.local/share/api-connect no se tocan)
# PREFIX cambia el destino (por defecto ~/.local).
set -eu

prefix=${PREFIX:-$HOME/.local}
apps=${XDG_DATA_HOME:-$HOME/.local/share}/applications
here=$(cd "$(dirname "$0")" && pwd)

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$prefix/bin/api-connect" "$prefix/bin/api-connect-cli" "$apps/api-connect.desktop"
    echo "api-connect desinstalado."
    exit 0
fi

if [ -x "$here/api-connect" ]; then
    bin=$here
else
    root=$(dirname "$here")
    echo "Compilando (cargo build --release)..."
    (cd "$root" && cargo build --release)
    bin=$root/target/release
fi

mkdir -p "$prefix/bin" "$apps"
install -m 755 "$bin/api-connect" "$bin/api-connect-cli" "$prefix/bin/"
cat > "$apps/api-connect.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=api-connect
Comment=Cliente de APIs REST agentic first
Exec=$prefix/bin/api-connect
Icon=applications-internet
Terminal=false
Categories=Development;Network;
DESKTOP

echo "Instalado en $prefix/bin (app: api-connect · consola: api-connect-cli)."
case ":$PATH:" in
    *":$prefix/bin:"*) ;;
    *) echo "Agrega $prefix/bin a tu PATH para usar api-connect-cli desde la terminal." ;;
esac

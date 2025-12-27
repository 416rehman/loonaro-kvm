#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VMS_DIR="$SCRIPT_DIR/../vms"
TEMPLATES_DIR="$SCRIPT_DIR/../templates"

list_templates() {
    echo "Available templates:"
    if [ -d "$TEMPLATES_DIR" ]; then
        for f in "$TEMPLATES_DIR"/*.xml; do
            if [ -f "$f" ]; then
                basename "$f" .xml
            fi
        done
    else
        echo "  (No templates directory found)"
    fi
}

# 1. Handle Arguments
TEMPLATE_ARG="$1"

# If no arg, --list, or help provided, show templates
if [ -z "$TEMPLATE_ARG" ] || [ "$TEMPLATE_ARG" == "--list" ] || [ "$TEMPLATE_ARG" == "-h" ]; then
    list_templates
    echo ""
    echo "Usage: $0 <template-name> [vm-name] [iso-path]"
    exit 0
fi

# 2. Validate Template
TEMPLATE_PATH="$TEMPLATES_DIR/${TEMPLATE_ARG}.xml"

if [ ! -f "$TEMPLATE_PATH" ]; then
    echo "Error: Template '$TEMPLATE_ARG' not found."
    list_templates
    exit 1
fi

VM_NAME="${2:-$TEMPLATE_ARG-sandbox}"
ISO_PATH="${3:-/home/ubuntu/shares/Downloads/ISOs/Win11_25H2_English_x64.iso}"

# Disable Libvirt Security Driver (AppArmor) for custom QEMU binary support
if ! grep -q '^security_driver = "none"' /etc/libvirt/qemu.conf; then
    echo "Disabling Libvirt security driver to allow custom QEMU binary..."
    echo 'security_driver = "none"' >> /etc/libvirt/qemu.conf
    systemctl restart libvirtd
fi

if [ ! -f "$ISO_PATH" ]; then
    echo "Warning: ISO not found at $ISO_PATH"
fi

DISK_SIZE="64G"

# Check if VM checks out
if sudo virsh list --all | grep -q "$VM_NAME"; then
    echo "Error: VM '$VM_NAME' already exists."
    exit 1
fi

mkdir -p "$VMS_DIR"

# Generate Polymorphic Identity
UUID=$(cat /proc/sys/kernel/random/uuid)
# Use a real Vendor OUI (e.g. ASUS: F0:2F:74) to avoid "52:54:00" QEMU signature
MAC="F0:2F:74:$(openssl rand -hex 3 | sed 's/\(..\)/\1:/g; s/.$//')"

# ASUS serials are often 15 chars, alphanumeric. e.g., R9N0...
SERIAL=$(tr -dc A-Z0-9 < /dev/urandom | head -c 15)
# NVMe/Disk serials are also alphanumeric, usually 12-20 chars
DISK_SN=$(tr -dc A-Z0-9 < /dev/urandom | head -c 12)

echo "Generating VM: $VM_NAME (Template: $TEMPLATE_ARG)"
echo "  UUID:   $UUID"
echo "  MAC:    $MAC"
echo "  Serial: $SERIAL"

# Create Disk
DISK_PATH="$VMS_DIR/$VM_NAME.qcow2"
# Force cleanup of disk if we are regenerating
if [ -f "$DISK_PATH" ] && [ "$VM_NAME" == "sandbox-001" ]; then
    rm -f "$DISK_PATH"
fi

if [ ! -f "$DISK_PATH" ]; then
    echo "Creating disk image..."
    qemu-img create -f qcow2 "$DISK_PATH" "$DISK_SIZE"
    
    # Fix permissions for Libvirt
    # Ensure Libvirt can traverse the home directory
    setfacl -m u:libvirt-qemu:rx "$HOME" 2>/dev/null || chmod +x "$HOME"
    chown libvirt-qemu:kvm "$DISK_PATH" 2>/dev/null || chown root:root "$DISK_PATH"
    chmod 600 "$DISK_PATH"
fi

# Generate XML
XML_PATH="$VMS_DIR/$VM_NAME.xml"
cp "$TEMPLATE_PATH" "$XML_PATH"

sed -i "s|REPLACE_VMS_DIR|$VMS_DIR|g" "$XML_PATH"
sed -i "s|REPLACE_ISO_PATH|$ISO_PATH|g" "$XML_PATH"
sed -i "s/REPLACE_NAME/$VM_NAME/g" "$XML_PATH"
sed -i "s/REPLACE_UUID/$UUID/g" "$XML_PATH"
sed -i "s/REPLACE_MAC/$MAC/g" "$XML_PATH"
sed -i "s/REPLACE_SERIAL/$SERIAL/g" "$XML_PATH"

# Setup NVRAM (UEFI Vars)
NVRAM_PATH="/var/lib/libvirt/qemu/nvram/${VM_NAME}_VARS.fd"
# Search for Microsoft-signed vars first
OVMF_VARS=$(find /usr/share -name "OVMF_VARS*ms.fd" | head -n 1)

if [ -z "$OVMF_VARS" ]; then
    # Fallback/Error handling
    if [ -f "/usr/share/OVMF/OVMF_VARS_4M.ms.fd" ]; then
         OVMF_VARS="/usr/share/OVMF/OVMF_VARS_4M.ms.fd"
    else
         echo "Error: OVMF_VARS (MS variant) not found."
         exit 1
    fi
fi

# Ensure NVRAM directory exists and we have permissions
sudo mkdir -p "$(dirname "$NVRAM_PATH")"
sudo cp "$OVMF_VARS" "$NVRAM_PATH"
sudo chmod 644 "$NVRAM_PATH"
sudo chown libvirt-qemu:kvm "$NVRAM_PATH" 2>/dev/null || true

# Define in Libvirt
echo "Defining VM in Libvirt..."
sudo virsh define "$XML_PATH"

echo "Done! Start your VM with: sudo virsh start $VM_NAME"

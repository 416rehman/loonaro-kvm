#!/bin/bash

# Default name if none provided
VM_NAME="${1:-sandbox-001}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VMS_DIR="$SCRIPT_DIR/../vms"
DISK_PATH="$VMS_DIR/$VM_NAME.qcow2"
XML_PATH="$VMS_DIR/$VM_NAME.xml"

echo "Tearing down VM: $VM_NAME"

# Check if VM exists in virsh
if sudo virsh list --all | grep -q "$VM_NAME"; then
    # Stop if running
    if sudo virsh list | grep -q "$VM_NAME"; then
        echo "  Stopping domain..."
        sudo virsh destroy "$VM_NAME"
    fi

    # Undefine and remove NVRAM
    echo "  Undefining domain and removing NVRAM..."
    sudo virsh undefine --nvram "$VM_NAME"
else
    echo "  Domain '$VM_NAME' not found in libvirt (skipping destroy/undefine)"
fi

# Remove Disk
if [ -f "$DISK_PATH" ]; then
    echo "  Removing disk image: $DISK_PATH"
    rm -f "$DISK_PATH"
else
    echo "  Disk image not found: $DISK_PATH"
fi

# Remove XML definition
if [ -f "$XML_PATH" ]; then
    echo "  Removing XML definition: $XML_PATH"
    rm -f "$XML_PATH"
else
    echo "  XML definition not found: $XML_PATH"
fi

# Remove JSON symlink
VM_JSON="$VMS_DIR/${VM_NAME}.json"
if [ -L "$VM_JSON" ]; then
    rm -f "$VM_JSON"
    echo "  Removed JSON symlink: $VM_JSON"
fi

echo "Teardown complete for $VM_NAME"

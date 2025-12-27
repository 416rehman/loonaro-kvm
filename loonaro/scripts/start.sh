#!/bin/bash
set -e

# Ensure we are running with sudo/root to access system libvirt
if [ "$EUID" -ne 0 ]; then 
  echo "Please run as root (sudo ./start_kvmi_vm.sh ...)"
  exit 1
fi
GUI=0

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --gui)
            GUI=1
            shift
            ;;
        *)
            VM_NAME="$1"
            shift
            ;;
    esac
done

if [ -z "$VM_NAME" ]; then
    echo "Usage: $0 [--gui] <vm-name>"
    exit 1
fi

# Start the VM if not running
if ! sudo virsh list --name | grep -q "^$VM_NAME$"; then
    echo "Starting VM: $VM_NAME"
    sudo virsh start "$VM_NAME"
else
    echo "VM $VM_NAME is already running."
fi

# Attach GUI if requested
if [ "$GUI" -eq 1 ]; then
    echo "Launching GUI viewer..."
    if command -v virt-viewer &> /dev/null; then
        # Run in background so script doesn't block, or foreground? 
        # User usually wants it to open. Let's run it.
        # virt-viewer --attach ...
        virt-viewer --attach "$VM_NAME" &
    else
        echo "Error: virt-viewer not found. Please install it (sudo apt install virt-viewer)."
        exit 1
    fi
fi

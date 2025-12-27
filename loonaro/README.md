# Loonaro: Stealthy KVM-VMI Sandbox

Loonaro is a specialized KVM virtualization wrapper designed for creating polymorphic, stealthy Windows 11 sandboxes with rapid spin-up and teardown capabilities.

## Directory Structure

*   `scripts/`: Management scripts for the VM lifecycle.
    *   `setup.sh`: Creates a new polymorphic VM instance.
    *   `teardown.sh`: Destroys and cleans up a VM instance.
    *   `start.sh`: Starts a VM and optionally launches the GUI.
*   `templates/`: XML definitions for the VMs.
*   `config/`: Networks and system configurations.
*   `vms/`: Runtime artifact storage (disk images, logs).

## Prerequisites

Before running Loonaro, ensure your system has the necessary KVM/QEMU and Libvirt dependencies installed.

```bash
# Core virtualization packages
sudo apt-get update
sudo apt-get install -y qemu-kvm libvirt-daemon-system libvirt-clients bridge-utils virt-manager

# AppArmor utilities (required for security bypass features in setup script)
sudo apt-get install -y apparmor-utils

# UEFI Firmware (OVMF) for Secure Boot support
sudo apt-get install -y ovmf

# TPM Emulator for Windows 11 support
sudo apt-get install -y swtpm swtpm-tools
```

## Quick Start

### 1. Create a VM
This script handles permissions, generates random hardware IDs, and configures Secure Boot.
```bash
# Usage: sudo ./scripts/setup.sh <vm-name> [path-to-windows-iso]
sudo ./scripts/setup.sh sandbox-001 /path/to/win11.iso
```

### 2. Start the VM
Starts the domain and launches `virt-viewer`.
```bash
sudo ./scripts/start.sh --gui sandbox-001
```

### 3. Teardown
Destroys the VM, removes the disk image, and undefines the domain/NVRAM.
```bash
sudo ./scripts/teardown.sh sandbox-001
```

## Requirements
*   Ubuntu 20.04+ (Tested on 24.04 Noble)
*   Sudo/Root privileges required for Libvirt interactions.

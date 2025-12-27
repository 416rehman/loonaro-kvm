# Loonaro-KVM

**OS-Agnostic Virtual Machine Orchestration with KVM-VMI Instrospection.**

This project separates the **Engine** (Hypervisor/QEMU) from the **Controller** (Loonaro Orchestration) to allow scalable, polymorphic VM management.

## Project Structure

- **`kvm-vmi/`**: (Submodule) The core engine. Contains theカスタム patched QEMU and KVM kernel modules for introspection.
- **`loonaro/`**: The controller. Contains templates, scripts, and configuration logic to spawn unique VM instances.

## Getting Started

### 1. Installation

Clone the repository recursively to fetch the KVM-VMI submodule:

```bash
git clone --recursive git@github.com:416rehman/loonaro-kvm.git
cd loonaro-kvm
```

### 2. Setup KVM-VMI Engine

Before you can run any VMs, you must compile and install the KVM-VMI patched kernel and QEMU.
Please follow the official [KVM-VMI Setup Guide](https://kvm-vmi.github.io/kvm-vmi/master/setup.html).

*Note: This usually involves building the linux kernel in `kvm-vmi/linux` and qemu in `kvm-vmi/qemu`.*

### 3. Setup Loonaro Controller

Once the engine is ready, you can start creating VMs.

#### List Available Templates
Check which VM templates are available (e.g., Windows 11, Linux, etc.):

```bash
cd loonaro
./scripts/setup.sh --list
```

#### Create a VM
Spawn a new VM instance with a unique polymorphic identity (UUID, MAC, Serial):

```bash
# Syntax: ./setup.sh <template> [vm-name] [iso-path]
./scripts/setup.sh win11 my-sandbox
```

If `[vm-name]` is omitted, it defaults to `<template>-sandbox`.

### 4. Adding New OS Support (e.g. Linux)
This architecture is OS-agnostic. To add support for a new operating system (e.g., Ubuntu):

1.  **Create a Template**: 
    Duplicate `loonaro/templates/win11.xml` to `loonaro/templates/linux.xml`.
    -   *Edit the Copy*: Remove Windows-specific features (like `<hyperv>`).
    -   *Update Boot*: Ensure the UEFI path (`OVMF_CODE`) matches your needs.

2.  **Run**:
    ```bash
    ./scripts/setup.sh linux ubuntu-vm
    ```

The `setup.sh` script automatically detects the new XML file in the `templates/` directory.

## Prerequisites
- **KVM-VMI Kernel**: Host must be running the patched kernel from `kvm-vmi`.
- **Libvirt**: Installed and running (`systemctl status libvirtd`).

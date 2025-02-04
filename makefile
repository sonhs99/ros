QEMU := qemu-system-x86_64 -m 512M -smp 1 -D debug.txt \
    -drive if=pflash,format=raw,readonly=on,file=OVMF_CODE.fd \
    -drive if=pflash,format=raw,readonly=on,file=OVMF_VARS.fd \
    -drive format=raw,file=fat:rw:esp \
	-hdb hdd.img \

QEMU_USB := -device qemu-xhci \
    -device usb-kbd \
    -device usb-mouse \

QEMU_TRACE := --trace "usb_xhci*"

QEMU_DEBUG := -S -gdb tcp::9000

.PHONY: build run trace dump hdd clean

esp/ap_bootstrap.bin: ap_bootstrap/entry.s
	nasm -o ./esp/ap_bootstrap.bin ./ap_bootstrap/entry.s

build: esp/ap_bootstrap.bin
	cargo -C ./kernel build --target x86_64.json --target-dir ../target -Z unstable-options
	cargo -C ./bootloader build --target x86_64-unknown-uefi --target-dir ../target -Z unstable-options
	cp ./target/x86_64/debug/kernel ./esp/kernel.elf
	cp ./target/x86_64-unknown-uefi/debug/bootloader.efi ./esp/efi/boot/bootx64.efi

run: build
	$(QEMU) $(QEMU_USB) -monitor stdio

run-without-usb: build
	$(QEMU) -monitor stdio

trace:
	$(QEMU) $(QEMU_USB) $(QEMU_TRACE)

dump:
	objdump -d ./target/x86_64/debug/kernel > dump.txt

hdd:
	qemu-img create hdd.img 20M

.PHONY: img
img:
	qemu-img create OS.img 200M
	mkfs.fat -n 'OS' -s 2 -f 2 -R32 -F 32 OS.img
	sudo mount OS.img mnt
	sudo mkdir -p mnt/efi/boot
	sudo cp esp/efi/boot/bootx64.efi mnt/efi/boot/bootx64.efi
	sudo cp esp/ap_bootstrap.bin mnt/ap_bootstrap.bin
	sudo cp esp/kernel.elf mnt/kernel.elf
	sudo umount mnt

debug:
	$(QEMU) $(QEMU_USB) $(QEMU_DEBUG)

clean:
	rm -rf target
	rm -rf esp
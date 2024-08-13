#![no_std]
#![no_main]

extern crate alloc;

use alloc::{boxed::Box, format, string::String, vec};
use core::{arch::asm, iter::empty, str};

use bootloader::{BootInfo, FrameBufferConfig, PixelFormat};
use kernel::{
    acpi,
    allocator::init_heap,
    console::{init_console, Console},
    device::{
        driver::keyboard::{get_code, getch, Keyboard},
        hdd::{
            pata::{get_device, init_pata},
            Block,
        },
        pci::{
            init_pci,
            msi::{Message, Msi},
            search::{Base, Interface, PciSearcher, Sub},
            switch_ehci_to_xhci, Pci, PciDevice,
        },
        xhc::{self, allocator::Allocator, regist_controller, register},
    },
    entry_point,
    float::set_ts,
    font::write_ascii,
    fs::{self, dev_list, format_by_name, init_fs, mount, open, open_dir},
    gdt::init_gdt,
    graphic::{get_graphic, init_graphic, GraphicWriter, PixelColor, PIXEL_WRITER},
    interrupt::{
        apic::{APICTimerMode, IOAPICRegister, LocalAPICId, LocalAPICRegisters},
        init_idt, set_interrupt, without_interrupts, InterruptVector,
    },
    ioapic,
    page::init_page,
    print, println,
    task::{create_task, exit, idle, init_task, running_task, schedule, TaskFlags},
    timer::init_pm,
};
use log::{debug, error, info, trace, warn};

entry_point!(kernel_main);

fn kernel_main(boot_info: BootInfo) {
    set_interrupt(false);
    let (height, width) = boot_info.frame_config.resolution();

    init_graphic(boot_info.frame_config);
    get_graphic().lock().clean();

    init_console(PixelColor::Black, PixelColor::White);

    info!("Rust Kernel Started");

    init_gdt();
    info!("GDT Initialized");

    init_idt();
    info!("IDT Initialized");

    init_page();
    info!("Page Table Initialized");

    init_heap(&boot_info.memory_map);
    info!("Heap Initialized");

    init_task();
    info!("Task Management Initialized");

    init_fs();
    info!("Root File System Initialized");

    // Do Not Use
    // set_ts();
    // info!("Lazy FP Enable");

    acpi::initialize(boot_info.rsdp);
    info!("ACPI Initialized");

    ioapic::init();
    info!("I/O APIC Initialized");

    init_pm();
    info!("ACPI PM Timer Initialized");

    LocalAPICRegisters::default().apic_timer().init(
        0b1011,
        false,
        APICTimerMode::Periodic,
        InterruptVector::APICTimer as u8,
    );
    set_interrupt(true);
    info!("Enable APIC Timer Interrupt");

    info!("PCI Init Started");
    init_pci();

    // create_task(TaskFlags::new(), test as u64, 0, 0);

    match PciSearcher::new()
        .base(Base::Serial)
        .sub(Sub::USB)
        .interface(Interface::XHCI)
        .search()
        .expect("No xHC device detected")
        .first()
    {
        Some(xhc_dev) => {
            info!(
                "xHC has been found: {}.{}.{}",
                xhc_dev.bus, xhc_dev.dev, xhc_dev.func
            );
            let xhc_bar = xhc_dev.read_bar(0);
            info!("Read BAR0: 0x{xhc_bar:016X}");
            let xhc_mmio_base = xhc_bar & (!0xfu64);
            info!("xHC MMIO base: 0x{xhc_mmio_base:016X}");

            if xhc_dev.read_vendor_id() == 0x8086 {
                switch_ehci_to_xhci(&xhc_dev);
            }

            without_interrupts(|| {
                let msg = Message::new()
                    .destionation_id(0xFF)
                    .interrupt_index(InterruptVector::XHCI as u8)
                    .level(true)
                    .trigger_mode(true)
                    .delivery_mode(0);
                xhc_dev.capabilities().for_each(|cap| {
                    debug!("Capability ID={:?}", cap.id());
                    if let Some(msi) = cap.msi() {
                        debug!("MSI Initialize Start");
                        msi.enable(&msg);
                        debug!("MSI Initialize Success");
                    } else if let Some(msi) = cap.msix() {
                        debug!("MSI-X Initialize Start");
                        msi.enable(&msg);
                        debug!("MSI-X Initialize Success");
                    }
                });

                let mut allocator = Allocator::new();
                let keyboard = Keyboard::new();
                let mut xhc: xhc::Controller<register::External, Allocator> =
                    xhc::Controller::new(xhc_mmio_base, allocator, vec![Box::new(keyboard.usb())])
                        .unwrap();
                xhc.reset_port().expect("xHCI Port Reset Failed");
                regist_controller(xhc);
            });
            create_task(TaskFlags::new(), print_input as u64, 0, 0);
        }
        None => {}
    }
    match PciSearcher::new()
        .base(Base::MassStorage)
        .sub(Sub::IDE)
        .interface(Interface::None)
        .search()
        .expect("No IDE device detected")
        .first()
    {
        Some(ide_dev) => {
            info!(
                "IDE has been found: {}.{}.{}",
                ide_dev.bus, ide_dev.dev, ide_dev.func
            );
            init_pata();
            for i in 0..4 {
                if i == 0 {
                    continue;
                }
                if let Ok(hdd) = get_device(i) {
                    info!("PATA:{i} Detected");
                    // create_task(TaskFlags::new(), test_hdd as u64, 0, 0);
                    let dev_name = format!("pata{i}");
                    if let Ok(fs_count) = mount(hdd, &dev_name) {
                        info!("PATA:{i} mounted, fs_count={fs_count}");
                        if fs_count == 0 {
                            if let Err(reason) = format_by_name(&dev_name, 1024 * 1024 * 10 / 512) {
                                info!("PATA:{i} format failed");
                                info!("{}", reason);
                            } else {
                                info!("PATA:{i} formated");
                            }
                        }
                    }
                    let mut count = 0;
                    let file = open(&dev_name, 0, "/file", b"w").expect("File Open Failed");
                    let root = open_dir(&dev_name, 0, "/", b"r")
                        .expect("Attempt to Open Root Directory Failed");
                    for (idx, entry) in root.entries() {
                        info!("[{idx}] /{entry}");
                        count += 1;
                    }
                    info!("Total {count} entries");
                } else {
                    info!("PATA:{i} Not Detected");
                }
            }

            for (idx, dev_name) in dev_list().iter().enumerate() {
                info!("[{idx}] {dev_name}");
            }
        }
        None => {}
    }
}

fn print_input() {
    loop {
        print!("{}", getch() as char);
    }
}

fn test_hdd() {
    let mut buffer: [Block<512>; 1] = [const { Block::empty() }; 1];
    let mut hdd = get_device(1).expect("Cannot find HDD");
    info!("PATA HDD Test Start");
    info!("1. Read");
    for lba in 0..4 {
        hdd.read_block(lba, &mut buffer).expect("HDD Read Failed");
        for (lba_offset, block) in buffer.iter().enumerate() {
            for idx in 0..512 {
                if idx % 16 == 0 {
                    print!(
                        "\nLBA={:2X}, offset={:3X}    |",
                        lba + lba_offset as u32,
                        idx
                    )
                }
                print!("{:02X} ", block.get::<u8>(idx));
            }
            println!();
        }
    }

    // info!("2. Write");
    // for block in buffer.iter_mut() {
    //     for idx in 0..512 {
    //         *block.get_mut(idx) = idx as u8;
    //     }
    // }
    // write_block(1, 0, &buffer).expect("HDD Write Failed");
    // read_block(1, 0, &mut buffer).expect("HDD Read Failed");
    // for (lba, block) in buffer.iter().enumerate() {
    //     for idx in 0..512 {
    //         if idx % 16 == 0 {
    //             print!("\nLBA={:2X}, offset={:3X}    |", lba, idx)
    //         }
    //         print!("{:02X} ", block.get::<u8>(idx));
    //     }
    //     println!();
    // }
}

fn test_hdd_rw() {
    let mut buffer: [Block<512>; 1] = [Block::empty(); 1];
    let mut pattern: [[Block<512>; 1]; 4] = [const { [Block::empty(); 1] }; 4];

    let hdd = get_device(1).expect("Cannot find HDD");

    for block in pattern[0].iter_mut() {
        for idx in 0..512 {
            *block.get_mut(idx) = idx as u8;
        }
    }

    for block in pattern[1].iter_mut() {
        for idx in 0..512 {
            *block.get_mut(idx) = (idx as u8) % 16;
        }
    }

    for block in pattern[2].iter_mut() {
        for idx in 0..512 {
            *block.get_mut(idx) = (idx as u8) % 2;
        }
    }

    for block in pattern[3].iter_mut() {
        for idx in 0..512 {
            if idx % 4 == 0 {
                *block.get_mut(idx) = 1;
            }
        }
    }

    let mut flag = false;
    info!("PATA HDD Read/Write Test Start");
    for (lba, pattern_buffer) in pattern.iter().enumerate() {
        info!("Pattern {}", lba + 1);
        hdd.write_block(lba as u32, pattern_buffer)
            .expect("HDD Write Failed");
        hdd.read_block(lba as u32, &mut buffer)
            .expect("HDD Read Failed");
        for idx in 0..512 {
            if *pattern_buffer[0].get::<u8>(idx) != *buffer[0].get::<u8>(idx) {
                flag = true;
                break;
            }
        }
        if flag {
            error!("Test Failed");
            for (pattern_block, block) in pattern_buffer.iter().zip(buffer.iter()) {
                for idx in 0..512 * 2 {
                    let offset = idx & 0x0F | (idx & !0x1F) >> 1;
                    if idx % 16 == 0 {
                        if idx % 32 == 0 {
                            print!("\nLBA={:2X}, offset={:3X}  |", lba as u32, idx)
                        } else {
                            print!(" |  ")
                        }
                    }
                    if (idx >> 4) & 0x01 == 0 {
                        print!("{:02X} ", block.get::<u8>(offset));
                    } else {
                        print!("{:02X} ", pattern_block.get::<u8>(offset));
                    }
                }
                println!();
            }
            return;
        }
    }
    info!("Test Success");
}

fn test() {
    for i in 0..50 {
        create_task(
            TaskFlags::new().thread().set_priority(66).clone(),
            test_thread as u64,
            0,
            0,
        );
    }
    for i in 0..50 {
        create_task(
            TaskFlags::new().thread().set_priority(130).clone(),
            test_thread as u64,
            0,
            0,
        );
    }
    for i in 0..50 {
        create_task(
            TaskFlags::new().thread().set_priority(200).clone(),
            test_thread as u64,
            0,
            0,
        );
    }
    loop {
        schedule();
    }
}

fn test_fpu() {
    let id = running_task().unwrap().id() + 1;
    let mut count = 1.0f64;

    for i in 0..10 {
        let before = count;
        let factor = (id + i) as f64 / id as f64;
        count *= factor;
        // info!("PID={:3}| count(mul)={:.6}", id, count);
        count /= factor;
        // info!("PID={:3}| count(div)={:.6}", id, count);
        if before != count {
            info!(
                "PID={:3}| Test Failed, before={:.6}, after={:.6}",
                id, before, count
            );
            return;
        }
    }
    info!("PID={:3}| Test Success", id);
}

fn test_thread() {
    let id = running_task().unwrap().id() + 1;
    let mut random = id;
    let mut value1 = 1f64;
    let mut value2 = 1f64;

    let data = [b'-', b'\\', b'|', b'/'];
    let offset = id * 2;
    let offset_x = id % 80 + 80;
    let offset_y = id / 80 + 25;
    let mut count = 0;

    loop {
        random = random * 1103515245 + 12345;
        random = (random >> 16) & 0xFFFF_FFFF;
        let factor = random % 255;
        let factor = (factor + id) as f64 / id as f64;
        value1 *= factor;
        value2 *= factor;

        if value1 != value2 {
            break;
        }

        value1 /= factor;
        value2 /= factor;

        if value1 != value2 {
            break;
        }

        write_ascii(
            offset_x * 8,
            offset_y * 16,
            data[count],
            PixelColor::Red,
            PixelColor::Black,
        );
        count = (count + 1) % 4;
    }
    write_ascii(
        offset_x * 8,
        offset_y * 16,
        b' ',
        PixelColor::Red,
        PixelColor::White,
    );
    info!("Thread id={id}: FPU Test Failed -> left={value1}, right={value2}");
}

fn test_windmill() {
    let id = running_task().unwrap().id() + 1;
    let data = [b'-', b'\\', b'|', b'/'];
    let offset = id * 2;
    let offset_x = id % 80 + 80;
    let offset_y = id / 80 + 25;
    let mut count = 0;

    loop {
        write_ascii(
            offset_x * 8,
            offset_y * 16,
            data[count],
            PixelColor::Red,
            PixelColor::Black,
        );
        count = (count + 1) % 4;
    }
}

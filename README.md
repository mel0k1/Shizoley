<div align="center">

<img src="assets/goblin.jpg" alt="Shizoley mascot" width="300"/>

# Shizoley

**экспериментальная ОС на микроядре Paparaci**

[![ci](https://github.com/mel0k1/Shizoley/actions/workflows/ci.yml/badge.svg)](https://github.com/mel0k1/Shizoley/actions/workflows/ci.yml)
[![license](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![rust](https://img.shields.io/badge/rust-nightly-orange.svg)](kernel)
[![arch](https://img.shields.io/badge/arch-x86__64-informational.svg)](kernel)

</div>

## Что это

**Shizoley** — хобби-операционная система, построенная по микроядерной схеме. Вся привилегированная логика живёт в микроядре **Paparaci**, написанном на **Rust**: загрузка, память, прерывания, syscall-интерфейс, вход в ring 3. Всё остальное (драйверы, сервисы, программы) — окружение на **C++**, которое уже сейчас исполняется в user mode и общается с ядром через узкий syscall-интерфейс на инструкции `syscall`.

Название ядру дано не случайно: Paparaci следит за всем, что происходит на машине, — как назойливый папарацци, только вместо фотографий он выдаёт page fault'ы.

### Архитектура

```
+----------------------------+  ring 3
|      userspace (C++20)     |
|   shz runtime, init, ...   |
+----------------------------+
     | syscall / sysret |
+----------------------------+  ring 0
|    paparaci kernel (Rust)  |
| gdt/idt/pic, paging, heap, |
| syscall msr, user loader   |
+----------------------------+
|        bootloader          |
+----------------------------+
```

### Текущее состояние

- [x] загрузка через bootloader (BIOS, x86_64)
- [x] вывод: VGA-текст 80x25 + COM1 (serial)
- [x] GDT/TSS, IDT, PIC с ремапом
- [x] обработка прерываний: таймер, клавиатура
- [x] page fault и double fault (через IST)
- [x] heap: bump-аллокатор + `alloc` API (`Vec`, `Box`, ...)
- [x] syscall-интерфейс (`syscall`/`sysret`, MSR STAR/LSTAR/SFMASK)
- [x] переход в ring 3: init исполняется в user mode
- [x] ELF-загрузчик: парсинг ELF64 (PT_LOAD, флаги R/W/X, BSS)
- [ ] планировщик задач
- [ ] драйверы, файловая система

## Структура

```
Shizoley/
├── kernel/                  # Paparaci — микроядро на Rust
│   ├── build.rs             # сборка init (g++) и вшивание статического ELF в ядро
│   ├── src/
│   │   ├── main.rs          # точка входа, panic handler
│   │   ├── vga.rs           # VGA-текстовый буфер 80x25
│   │   ├── serial.rs        # логи через COM1 (uart_16550)
│   │   ├── gdt.rs           # GDT: kernel/user сегменты, TSS (IST + RSP0)
│   │   ├── interrupts.rs    # IDT, PIC, таймер, клавиатура
│   │   ├── memory.rs        # таблицы страниц, frame-аллокатор
│   │   ├── allocator.rs     # heap (bump-аллокатор), GlobalAlloc
│   │   ├── elf.rs           # ELF64-загрузчик: PT_LOAD, флаги R/W/X, BSS
│   │   ├── syscall.rs       # MSR + asm-трамплин syscall/sysret + диспетчер
│   │   └── user.rs          # маппинг стека init и вход в ring 3
│   ├── x86_64-paparaci.json # кастомный target (без red zone)
│   └── .cargo/config.toml
├── userspace/               # окружение на C++20 (freestanding)
│   ├── include/shizoley/    # syscall ABI, io, типы
│   ├── src/                 # рантайм: syscall-обёртки, вывод, _start
│   ├── programs/init/       # init — первая программа в ring 3
│   └── link.ld              # фиксация образа на 0x400000
├── .github/workflows/       # CI: сборка + линт
└── assets/                  # маскот
```

## Сборка

Требования: `rustup` (nightly + `rust-src` + `llvm-tools-preview`), `bootimage`, `cmake`, `g++`, опционально `qemu-system-x86_64`.

```bash
# инструменты
rustup toolchain install nightly --profile minimal \
    --component rust-src,llvm-tools-preview,clippy,rustfmt
cargo install bootimage

# ядро
cd kernel
cargo build          # проверка сборки
cargo bootimage      # загрузочный образ .bin

# окружение (C++)
cmake -S userspace -B userspace/build
cmake --build userspace/build
```

Или всё сразу из корня: `make build`.

## Запуск

```bash
cd kernel && cargo run
# эквивалентно:
qemu-system-x86_64 -drive format=raw,file=target/x86_64-paparaci/debug/bootimage-paparaci.bin -serial stdio
```

Ядро пишет баннер в VGA и serial, проверяет heap через `alloc::Vec`, затем парсит вшитый статический ELF `init`: `elf.rs` валидирует заголовок, раскладывает `PT_LOAD`-сегменты по их `p_vaddr` с флагами из `p_flags` (W → writable, не-X → NX), затирает BSS, мапит стек и уходит в ring 3 через `iretq`. Дальше работает userspace: `init` печатает баннер и `3^10`, делая syscall `putc`; на `exit` ядро печатает код выхода и засыпает в `hlt`. Клавиатура по-прежнему жива — нажатия печатаются на экран из обработчика прерывания.

## Syscall ABI

| nr | имя    | аргументы     | описание             |
|----|--------|---------------|----------------------|
| 0  | `putc` | `rdi`: символ | символ в консоль     |
| 1  | `exit` | `rdi`: код    | завершить исполнение |

Конвенция: номер в `rax`, аргументы в `rdi`, `rsi`, `rdx`, результат в `rax`.

Как это устроено:

- userspace вызывает инструкцию `syscall`; ядро заранее настраивает MSR: `EFER.SCE`, `STAR` (kernel `0x08`/`0x10`, sysret попадает на user `0x20`/`0x18` через смещения +16/+8), `LSTAR` → трамплин, `SFMASK` сбрасывает TF/IF/DF/IOPL/NT;
- трамплин на asm переключается на отдельный syscall-стек, сохраняет GPR и пользовательский RSP, вызывает диспетчер на Rust, восстанавливает состояние и возвращается `sysretq`;
- прерывания из ring 3 обслуживаются через `TSS.RSP0`; страницы пользователя замаплены с `USER_ACCESSIBLE`, user-бит проставлен и на таблицах-предках.

## Дорожная карта

1. Планировщик и потоки в Paparaci (адресные пространства на процесс, загрузка ELF из initrd/ФС).
2. Драйверы и ФС в user space — как и положено микроядру.

## Лицензия

Apache-2.0 — см. [LICENSE](LICENSE).

<details>
<summary>маскот в ascii</summary>

```
                                                     .::.                
                                                   .:#*:.                
                                                 .:*@@::.                
                                               ..*@@@@:.                 
                                              .-%@@@@@:.                
                                           .::#@@@@@@@.                 
                                         .:#@@@@@@@@@*.                 
         .:-::::..          :.  .:...::.....:*@@@@@@@@@@:.                  
          -%@%#*-::::......  ::::::=#::######@@@@@@@@#--:                   
           :*%@@@@@@@@@@@%=----:%@-:#+:+@@@@@@@@@@@%:...                    
             .:*@@@@@@@@@@@@@@@=:+%*%@%@@@+++%@@@@@@+:..                    
               .:*@@@@@@@@@@@@@@@@@@@@@@+:::::#@@@@@@%-:                    
                 .:#@@@@@@@@@@@@@@@@@@@#:#@@#=::@@@@@#:.                    
                   :-*@@@@@@@@@@@@@@@@%::::-@@-%@@@@@:.                     
                     .:@@*:::-#%@@@@@%:::::::-%@#:*@@:.                     
                     .:@@%=::-##=:#@@::::*##@@@@::#@@:.                     
                     .:@@@+:::-#@@@@@@@@@@@%##%%:+@@@=.                     
                      .-:-*%@@@@@%@@@@@@@@:...+**#@@@-.                     
                    ::=@@@@@@%-:::@@@@@@@=:%-:%@@@@@#.                      
                 .::***+=-::=+#=:%%%%%##=:@@@@@@@%*::                       
                          .-@@@=:::::::-+%@@@@@@-:.                         
                           .:-%@**-++-**%@@#+::..                           
                             ..=*::-+*-=-:..                                
                               =*:::+*                                      
                              :+=:::+*                                      
                             =*=:::=**                                      
                            **+=+:-*+                                       
                           :**=:+**=                                        
                                =+.                                         
```

</details>

---

<div align="center">

# Shizoley (EN)

</div>

**Shizoley** is a hobby operating system built on a microkernel design. All privileged logic lives in the **Paparaci** microkernel written in **Rust**: boot, memory management, interrupts, the syscall interface, and the ring 3 transition. Everything else (drivers, services, programs) is the **C++** environment, which already runs in user mode and talks to the kernel through a narrow `syscall`-instruction interface.

The kernel is named after paparazzi for a reason: Paparaci watches everything happening on the machine — like an annoying photographer, except instead of photos it hands out page faults.

### Current status

- [x] Boot via bootloader (BIOS, x86_64)
- [x] Output: VGA text 80x25 + COM1 serial
- [x] GDT/TSS, IDT, remapped PIC
- [x] Interrupts: timer, keyboard
- [x] Page fault and double fault (via IST)
- [x] Heap: bump allocator + `alloc` API (`Vec`, `Box`, ...)
- [x] Syscall interface (`syscall`/`sysret`, MSR STAR/LSTAR/SFMASK)
- [x] Ring 3: init runs in user mode
- [x] ELF loader: ELF64 parsing (PT_LOAD, R/W/X flags, BSS)
- [ ] Task scheduler
- [ ] Drivers, file system

### Build & run

Requirements: `rustup` (nightly + `rust-src` + `llvm-tools-preview`), `bootimage`, `cmake`, `g++`, optionally `qemu-system-x86_64`.

```bash
cd kernel
cargo build          # build check
cargo bootimage      # bootable image

cmake -S userspace -B userspace/build
cmake --build userspace/build

cd kernel && cargo run   # boot in QEMU
```

The kernel prints a banner to VGA and serial, sanity-checks the heap with `alloc::Vec`, then parses the embedded static ELF of `init`: `elf.rs` validates the header, lays out `PT_LOAD` segments at their `p_vaddr` with flags derived from `p_flags` (W → writable, non-X → NX), zero-fills BSS, maps the stack and drops into ring 3 via `iretq`. From that point userspace is in charge: `init` prints a banner and `3^10` through the `putc` syscall; on `exit` the kernel prints the exit code and sleeps in `hlt`. The keyboard still works — keypresses are echoed from the interrupt handler.

### Syscall ABI

| nr | name   | args          | description         |
|----|--------|---------------|---------------------|
| 0  | `putc` | `rdi`: char   | put char to console |
| 1  | `exit` | `rdi`: code   | terminate execution |

Convention: number in `rax`, args in `rdi`, `rsi`, `rdx`, result in `rax`.

How it works:

- userspace issues the `syscall` instruction; the kernel pre-configures the MSRs: `EFER.SCE`, `STAR` (kernel `0x08`/`0x10`, sysret lands on user `0x20`/`0x18` via the +16/+8 offsets), `LSTAR` → trampoline, `SFMASK` clears TF/IF/DF/IOPL/NT;
- the asm trampoline switches to a dedicated syscall stack, saves GPRs and the user RSP, calls the Rust dispatcher, restores state and returns with `sysretq`;
- interrupts from ring 3 are served through `TSS.RSP0`; user pages are mapped with `USER_ACCESSIBLE`, and the user bit is also set on ancestor tables.

### License

Apache-2.0 — see [LICENSE](LICENSE).

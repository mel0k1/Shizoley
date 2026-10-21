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
|  shz runtime, init, ...    |
+----------------------------+
     | syscall / sysret |
+----------------------------+  ring 0
|    paparaci kernel (Rust)  |
| gdt/idt/pic, paging, heap, |
| tasks: CR3 на задачу,      |
| планировщик, потоки,       |
| syscall msr, elf loader    |
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
- [x] адресное пространство на задачу: свой CR3, глубокая копия ветки ядра
- [x] планировщик round-robin: вытеснение по таймеру (PIT 100 Гц) + yield
- [x] потоки: контекст-свитч с fxsave, per-task kernel stack (syscall + RSP0)
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
│   │   ├── memory.rs        # таблицы страниц, глобальный frame-аллокатор
│   │   ├── allocator.rs     # heap (bump-аллокатор), GlobalAlloc
│   │   ├── task.rs          # AddressSpace (CR3 на задачу), Task, __switch
│   │   ├── sched.rs         # ready-очередь, вытеснение, yield, exit, reap
│   │   ├── elf.rs           # ELF64-загрузчик: PT_LOAD, флаги R/W/X, BSS
│   │   ├── syscall.rs       # MSR + asm-трамплин syscall/sysret + диспетчер
│   │   └── user.rs          # spawn процесса в новое адресное пространство и вход в ring 3
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

Ядро бутится, поднимает heap и MSR, затем планировщик: `main` становится задачей 0, рядом стартуют два потока ядра (`alpha`, `beta`) — каждый печатает пять тиков и завершается, чередуясь по таймеру (PIT 100 Гц, round-robin). Параллельно `elf.rs` раскладывает вшитый статический ELF `init` в **свежее адресное пространство** (свой CR3: верхняя половина ядра разделяется, нижняя — глубокая копия), мапит стек и ставит задачу в очередь. Когда планировщик отдает ей CPU, трамплин через `iretq` уходит в ring 3; `init` печатает баннер и `3^10` через syscall `putc`, на `exit` планировщик снимает задачу, а main-петля-жнец печатает код выхода. Page fault из ring 3 убивает только провинившуюся задачу — ядро продолжает жить. Клавиатура по-прежнему жива — нажатия печатаются на экран из обработчика прерывания.

## Syscall ABI

| nr | имя    | аргументы     | описание             |
|----|--------|---------------|----------------------|
| 0  | `putc` | `rdi`: символ | символ в консоль     |
| 1  | `exit` | `rdi`: код    | завершить исполнение |

Конвенция: номер в `rax`, аргументы в `rdi`, `rsi`, `rdx`, результат в `rax`.

Как это устроено:

- userspace вызывает инструкцию `syscall`; ядро заранее настраивает MSR: `EFER.SCE`, `STAR` (kernel `0x08`/`0x10`, sysret попадает на user `0x20`/`0x18` через смещения +16/+8), `LSTAR` → трамплин, `SFMASK` сбрасывает TF/IF/DF/IOPL/NT;
- трамплин на asm переключается на kernel-стек **текущей задачи** (планировщик обновляет его при каждом свитче), сохраняет GPR и пользовательский RSP, вызывает диспетчер на Rust, восстанавливает состояние и возвращается `sysretq`;
- прерывания из ring 3 обслуживаются через `TSS.RSP0`, который планировщик ретаргетит на kernel-стек запускаемой задачи; страницы пользователя замаплены с `USER_ACCESSIBLE`, user-бит проставлен и на таблицах-предках.

## Задачи и адресные пространства

- **AddressSpace** — свой CR3 на задачу: рамка корня нового пространства обнуляется, верхняя половина (physmap и всё ядро) шарится дословно, нижняя — глубокая копия таблиц с общими листовыми кадрами, поэтому память ядра доступна из ring 0 при любом активном CR3, но закрыта USER-битом на листьях для ring 3;
- **Task** — kernel-стек (32 КиБ из heap), `SwitchFrame` (fxsave 512 Б + rsp), точка входа; потоки ядра шарят адресное пространство ядра, у процесса оно своё;
- **__switch** — сохранение callee-saved + fxsave/fxrstor + перескок на стек следующей задачи; задача, прерванная прямо внутри обработчика IRQ или syscall, доезжает до конца и корректно уходит через `iretq`/`sysret`;
- планировщик — round-robin: таймер (PIT 100 Гц) кладёт текущую задачу в конец очереди и переключает, `yield` делает то же кооперативно; завершившаяся задача попадает к жнецу в main-петле, где печатается её код выхода и освобождается kernel-стек.

## Дорожная карта

1. IPC в Paparaci (микроядро обязано уметь передавать сообщения), ELF из initrd/ФС, user-потоки одного процесса.
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
- [x] Per-task address spaces: own CR3, deep-copied kernel branch
- [x] Round-robin scheduler: timer preemption (PIT 100 Hz) + yield
- [x] Threads: context switch with fxsave, per-task kernel stack (syscall + RSP0)
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

The kernel boots, sets up the heap and MSRs, then hands control to the scheduler: `main` becomes task 0, and two kernel threads (`alpha`, `beta`) start next to it — each prints five ticks and finishes, interleaved by the timer (PIT 100 Hz, round-robin). Meanwhile `elf.rs` lays the embedded static ELF of `init` out into a **fresh address space** (its own CR3: the upper kernel half is shared verbatim, the lower half is a deep copy), maps the stack and queues the task. When the scheduler hands it a CPU, the trampoline drops into ring 3 via `iretq`; `init` prints a banner and `3^10` through the `putc` syscall; on `exit` the scheduler retires the task and the reaper loop in `main` prints its exit code. A page fault from ring 3 kills only the offending task — the kernel lives on. The keyboard still works — keypresses are echoed from the interrupt handler.

### Syscall ABI

| nr | name   | args          | description         |
|----|--------|---------------|---------------------|
| 0  | `putc` | `rdi`: char   | put char to console |
| 1  | `exit` | `rdi`: code   | terminate execution |

Convention: number in `rax`, args in `rdi`, `rsi`, `rdx`, result in `rax`.

How it works:

- userspace issues the `syscall` instruction; the kernel pre-configures the MSRs: `EFER.SCE`, `STAR` (kernel `0x08`/`0x10`, sysret lands on user `0x20`/`0x18` via the +16/+8 offsets), `LSTAR` → trampoline, `SFMASK` clears TF/IF/DF/IOPL/NT;
- the asm trampoline switches to the **current task's** kernel stack (the scheduler retargets it on every switch), saves GPRs and the user RSP, calls the Rust dispatcher, restores state and returns with `sysretq`;
- interrupts from ring 3 are served through `TSS.RSP0`, which the scheduler retargets to the incoming task's kernel stack; user pages are mapped with `USER_ACCESSIBLE`, and the user bit is also set on ancestor tables.

## Tasks and address spaces

- **AddressSpace** — one CR3 per task: the new root frame is zeroed, the upper half (physmap and the whole kernel) is shared verbatim, the lower half is a deep copy of the tables with shared leaf frames, so kernel memory stays reachable from ring 0 under any active CR3, while ring 3 is locked out by supervisor-only leaves;
- **Task** — a kernel stack (32 KiB from the heap), a `SwitchFrame` (512-byte fxsave + rsp), an entry point; kernel threads share the kernel address space, a process gets its own;
- **__switch** — callee-saved registers + fxsave/fxrstor + a jump onto the next task's stack; a task interrupted right inside an IRQ or syscall handler finishes it later and leaves through `iretq`/`sysret` correctly;
- the scheduler is round-robin: the timer (PIT 100 Hz) requeues the current task and switches, `yield` does the same cooperatively; a finished task goes to the reaper in the main loop, which prints its exit code and frees the kernel stack.

### License

Apache-2.0 — see [LICENSE](LICENSE).

### Roadmap

1. IPC in Paparaci (a microkernel must pass messages), ELF from initrd/FS, user threads of one process.
2. Drivers and FS in user space — the way a microkernel should.

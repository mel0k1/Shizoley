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
|  shz runtime, init, hello  |
+----------------------------+
     | syscall / sysret |
+----------------------------+  ring 0
|    paparaci kernel (Rust)  |
| gdt/idt/pic, paging, heap, |
| процессы и user-потоки,    |
| mailbox IPC, initrd,       |
| планировщик, syscall msr,  |
| elf loader                 |
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
- [x] процессы: Process (pid, имя, mailbox) + user-потоки одного адресного пространства
- [x] initrd: ustar-архив, собираемый build.rs из programs/*, spawn по имени
- [x] mailbox IPC: ipc_send/ipc_recv с блокировкой получателя и прямой доставкой
- [ ] драйверы, файловая система
- [ ] асинхронная очередь ядро->процесс (по мотивам HelQueue из managarm)

## Структура

```
Shizoley/
├── kernel/                  # Paparaci — микроядро на Rust
│   ├── build.rs             # сборка programs/* (g++) и упаковка их в ustar-initrd
│   ├── src/
│   │   ├── main.rs          # точка входа, panic handler
│   │   ├── vga.rs           # VGA-текстовый буфер 80x25
│   │   ├── serial.rs        # логи через COM1 (uart_16550)
│   │   ├── gdt.rs           # GDT: kernel/user сегменты, TSS (IST + RSP0)
│   │   ├── interrupts.rs    # IDT, PIC, таймер, клавиатура
│   │   ├── memory.rs        # таблицы страниц, глобальный frame-аллокатор
│   │   ├── allocator.rs     # heap (bump-аллокатор), GlobalAlloc
│   │   ├── task.rs          # AddressSpace (CR3 на процесс), Process, Task, __switch
│   │   ├── sched.rs         # ready-очередь, парковка в recv, exit, reap
│   │   ├── ipc.rs           # mailbox процессов: send/recv, registry процессов
│   │   ├── initrd.rs        # парсер ustar: lookup(name) -> elf-образ
│   │   ├── elf.rs           # ELF64-загрузчик: PT_LOAD, флаги R/W/X, BSS
│   │   ├── syscall.rs       # MSR + asm-трамплин syscall/sysret + диспетчер
│   │   └── user.rs          # spawn процесса из initrd, вход в ring 3
│   ├── x86_64-paparaci.json # кастомный target (без red zone)
│   └── .cargo/config.toml
├── userspace/               # окружение на C++20 (freestanding)
│   ├── include/shizoley/    # syscall ABI, io, типы
│   ├── src/                 # рантайм: syscall-обёртки, вывод, _start
│   ├── programs/init/       # init: user-потоки + spawn + IPC ping-pong
│   ├── programs/hello/      # hello: второй процесс, участник IPC-демо
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

Ядро бутится, поднимает heap и MSR, затем планировщик: `main` становится задачей 0, рядом стартует kernel-поток `heartbeat` (доказывает, что вытеснение живёт, пока init блокирован в IPC). Параллельно build.rs кладёт все `programs/*` в ustar-initrd, вшитый в ядро; `spawn_process("init")` находит образ, `elf.rs` раскладывает его в **свежее адресное пространство** (свой CR3) и ставит процесс в очередь. Дальше init в ring 3:

- создаёт **два user-потока** (`thread_create`) — оба работают в одном адресном пространстве, печатают тики и завершаются;
- делает `spawn("hello")` — ядро грузит второй ELF из initrd в отдельный процесс (pid 2);
- шлёт `ping` через `ipc_send`, `hello` ловит его блокирующим `ipc_recv`, отвечает `pong` — полный ping-pong между двумя процессами с разными CR3;
- `exit` всех потоков процесса завершает процесс, жнец печатает коды выходов.

Page fault из ring 3 убивает только провинившуюся задачу — ядро продолжает жить. Клавиатура по-прежнему жива — нажатия печатаются на экран из обработчика прерывания.

## Syscall ABI

| nr | имя             | аргументы                              | описание                                   |
|----|-----------------|----------------------------------------|--------------------------------------------|
| 0  | `putc`          | `rdi`: символ                          | символ в консоль                           |
| 1  | `exit`          | `rdi`: код                             | завершить поток; процесс умирает с последним потоком |
| 2  | `spawn`         | `rdi`: имя, `rsi`: длина               | ELF из initrd -> новый процесс, -> pid     |
| 3  | `thread_create` | `rdi`: entry, `rsi`: stack top, `rdx`: arg | user-поток в текущем процессе, -> tid  |
| 4  | `ipc_send`      | `rdi`: pid, `rsi`: buf, `rdx`: len     | сообщение в mailbox или напрямую ждущему   |
| 5  | `ipc_recv`      | `rdi`: src (0 = любой), `rsi`: buf, `rdx`: max | блокируется до сообщения, -> sender pid |
| 6  | `getpid`        | —                                      | pid текущего процесса                      |
| 7  | `gettid`        | —                                      | tid текущего потока                        |

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

## Процессы, потоки и IPC

- **Process** — pid, имя, `AddressSpace` (Mutex для мутаций страниц), mailbox, список ждущих recv-потоков и счётчик живых потоков; CR3 хранится отдельно и неизменно, поэтому свитч читает его без блокировки; процесс живёт, пока жив хоть один его поток (Arc);
- **user-потоки одного процесса** — `thread_create` создаёт Task, разделяющий адресное пространство и mailbox процесса; стек потока — обычная память пользователя (init даёт потоки статические .bss-буферы), аргумент приходит в `rdi`;
- **mailbox IPC** — `ipc_send` копирует данные из адресного пространства отправителя и либо отдаёт их напрямую первому подходящему припаркованному получателю (просыпаясь с `sender pid` как результатом recv), либо кладёт в mailbox получателя (cap 32 сообщения, payload до 4 КиБ); копии идут через physmap по translate, без переключения CR3;
- **initrd** — build.rs компилирует каждый `programs/<name>/main.cpp` с рантаймом `shz` в статический ELF и упаковывает их в ustar-архив с корректной контрольной суммой; ядро вшивает архив и раздаёт образы по имени на `spawn`.

## Дорожная карта

1. Асинхронная очередь ядро->процесс в общей памяти (по мотивам HelQueue из managarm) как «второй помощник» простому mailbox IPC.
2. Драйверы и ФС в user space — как и положено микроядру; initrd уже даёт базу для spawn'а сервисов.
3. Дальше — по обстоятельствам: подсматриваем у managarm (C++, LAN/потоковый IPC, universe-хэндлы) и Veda (Rust, capability-каналы, свой VT-x гипервизор с EPT+IOMMU для драйвер-Linux VM).

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
- [x] Processes: Process (pid, name, mailbox) + user threads sharing one address space
- [x] initrd: ustar archive built by build.rs from programs/*, spawn by name
- [x] Mailbox IPC: ipc_send/ipc_recv with blocking receiver and direct handoff
- [ ] Drivers, file system
- [ ] Async kernel->process queue (inspired by managarm's HelQueue)

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

The kernel boots, sets up the heap and MSRs, then hands control to the scheduler: `main` becomes task 0, and a kernel thread `heartbeat` starts next to it (proving preemption keeps running while init blocks in IPC). Meanwhile build.rs packs every `programs/*` into a ustar initrd embedded into the kernel; `spawn_process("init")` looks the image up, `elf.rs` lays it out into a **fresh address space** (its own CR3) and queues the process. Then init runs in ring 3:

- creates **two user threads** (`thread_create`) — both share the process address space, print ticks and finish;
- calls `spawn("hello")` — the kernel loads the second ELF from the initrd into a separate process (pid 2);
- sends `ping` via `ipc_send`; `hello` catches it with a blocking `ipc_recv` and replies `pong` — a full ping-pong between two processes with different CR3s;
- when the last thread of a process exits, the process is done and the reaper prints the exit codes.

A page fault from ring 3 kills only the offending task — the kernel lives on. The keyboard still works — keypresses are echoed from the interrupt handler.

### Syscall ABI

| nr | name            | args                                        | description                                        |
|----|-----------------|---------------------------------------------|----------------------------------------------------|
| 0  | `putc`          | `rdi`: char                                 | put char to console                                |
| 1  | `exit`          | `rdi`: code                                 | end the thread; the process dies with its last thread |
| 2  | `spawn`         | `rdi`: name, `rsi`: len                     | ELF from initrd -> new process, -> pid             |
| 3  | `thread_create` | `rdi`: entry, `rsi`: stack top, `rdx`: arg  | user thread in the current process, -> tid         |
| 4  | `ipc_send`      | `rdi`: pid, `rsi`: buf, `rdx`: len          | message to a mailbox or straight to a waiting recv |
| 5  | `ipc_recv`      | `rdi`: src (0 = any), `rsi`: buf, `rdx`: max | blocks until a message, -> sender pid             |
| 6  | `getpid`        | —                                           | pid of the current process                         |
| 7  | `gettid`        | —                                           | tid of the current thread                          |

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

## Processes, threads and IPC

- **Process** — pid, name, an `AddressSpace` (Mutex for page-table mutations), a mailbox, the list of parked recv-threads and a live-thread counter; CR3 is stored separately and never changes, so the switch path reads it lock-free; a process lives while any of its threads holds an Arc;
- **user threads of one process** — `thread_create` creates a Task sharing the process address space and mailbox; the thread stack is ordinary user memory (init hands out static .bss buffers), the argument arrives in `rdi`;
- **mailbox IPC** — `ipc_send` copies the payload out of the sender's address space and either hands it straight to the first matching parked receiver (waking it with `sender pid` as the recv result) or drops it into the target mailbox (cap 32 messages, payload up to 4 KiB); copies go through the physmap via translate, no CR3 switch needed;
- **initrd** — build.rs compiles every `programs/<name>/main.cpp` against the `shz` runtime into a static ELF and packs them into a ustar archive with a correct checksum; the kernel embeds the archive and serves images by name on `spawn`.

### Roadmap

1. An async kernel->process queue in shared memory (inspired by managarm's HelQueue) as the "second helper" next to the simple mailbox IPC.
2. Drivers and FS in user space — the way a microkernel should; the initrd already gives a base for spawning services.
3. Peek further at managarm (C++, lane/stream IPC, universe handles) and Veda (Rust, capability channels, an own VT-x hypervisor with EPT+IOMMU for a driver-Linux VM).

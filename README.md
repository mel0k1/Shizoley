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
- [x] event queue: managarm-подобная очередь ядро->процесс в памяти пользователя (doorbell, chunk-кредиты, eq_wait/eq_kick)
- [x] сервисы в ring 3: fsd (файлы из initrd по mailbox) и rtcd (CMOS-часы через порты 0x70/0x71, IOPL=3)
- [x] initrd разделяется в каждый процесс read-only (0x20000000), парсится прямо в user space
- [x] begin гипервизора: probe VT-x, CR4.VMXE, VMXON, чтение capability-MSR (vmx.rs)
- [ ] драйверы: клавиатура/консоль в user space через event queue
- [ ] гипервизор: VMCS, vcpu, guest по Veda-рецепту (GUEST_CREATE/VCPU_RUN)

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
│   │   ├── eq.rs            # event queue: chunk-кольцо в памяти пользователя, doorbell
│   │   ├── initrd.rs        # парсер ustar: lookup(name) -> elf-образ (page-aligned блоб)
│   │   ├── elf.rs           # ELF64-загрузчик: PT_LOAD, флаги R/W/X, BSS
│   │   ├── syscall.rs       # MSR + asm-трамплин syscall/sysret + диспетчер
│   │   ├── user.rs          # spawn процесса из initrd, RO-маппинг initrd, вход в ring 3
│   │   └── vmx.rs           # гипервизор begin: VT-x probe, VMXON, capability MSRs
│   ├── x86_64-paparaci.json # кастомный target (без red zone)
│   └── .cargo/config.toml
├── userspace/               # окружение на C++20 (freestanding)
│   ├── include/shizoley/    # syscall ABI, io, типы
│   ├── src/                 # рантайм: syscall-обёртки, вывод, _start
│   ├── programs/init/       # init: потоки, ping-pong, сервисы, event queue
│   ├── programs/hello/      # hello: 100 событий через event queue
│   ├── programs/fsd/        # файловый сервис: ustar из RO-маппинга initrd
│   ├── programs/rtcd/       # драйвер часов: CMOS-порты из ring 3 (IOPL=3)
│   ├── rootfs/motd          # данные для fsd
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
- делает `spawn("hello")` — ядро грузит второй ELF из initrd в отдельный процесс (pid 2) и гоняет с ним ping-pong через mailbox IPC;
- поднимает **сервисы в ring 3**: `fsd` (отдаёт файл `motd` из RO-маппинга initrd по mailbox) и `rtcd` (читает CMOS-часы портами 0x70/0x71 — IOPL=3);
- регистрирует **event queue** (`eq_setup`), hello постит 100 событий через `eq_post` — init вычитывает их обычными load'ами из памяти (без syscall на элемент), между пачками спя в ядре на doorbell (`eq_wait`);
- `exit` последнего потока завершает процесс, жнец печатает коды выходов.

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
| 8  | `eq_setup`      | `rdi`: база, `rsi`: размер             | зарегистрировать event queue, -> qid       |
| 9  | `eq_post`       | `rdi`: qid, `rsi`: opcode, `rdx`: buf, `r10`: len | элемент в очередь + doorbell      |
| 10 | `eq_wait`       | `rdi`: qid                             | спать до doorbell, -> 1                    |
| 11 | `eq_kick`       | `rdi`: qid                             | отдать chunk-кредит припаркованным постерам |
| 12 | `initrd_size`   | —                                      | размер initrd (RO-маппинг на 0x20000000)   |
| 13 | `write`         | `rdi`: buf, `rsi`: len                 | атомарная строка в консоль (vga+serial)    |

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

1. Гипервизор: VMCS + vcpu + EPT, syscalls GUEST_CREATE/VCPU_RUN (Veda-style driver VM).
2. Клавиатурный/консольный драйверы в user space на event queue; права на qid как capability (вместо сырых номеров).
3. Подсматриваем дальше: managarm (universe-хэндлы, UAR/STAC-копии) и Veda (would_cycle-детект capability-каналов, IOMMU для гостевых устройств).

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
- [x] Event queue: managarm-style kernel->process queue in user memory (doorbell, chunk credits, eq_wait/eq_kick)
- [x] Ring 3 services: fsd (initrd files over mailbox) and rtcd (CMOS clock via ports 0x70/0x71, IOPL=3)
- [x] initrd shared read-only into every process (0x20000000), parsed in user space
- [x] Hypervisor begin: VT-x probe, CR4.VMXE, VMXON, capability MSRs (vmx.rs)
- [ ] Drivers: keyboard/console in user space over the event queue
- [ ] Hypervisor: VMCS, vcpu, guest the Veda way (GUEST_CREATE/VCPU_RUN)

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
- calls `spawn("hello")` — the kernel loads the second ELF from the initrd into a separate process (pid 2) and runs a mailbox ping-pong with it;
- brings up **ring 3 services**: `fsd` (serves the `motd` file from the read-only initrd mapping over the mailbox) and `rtcd` (reads the CMOS clock through ports 0x70/0x71 — IOPL=3);
- registers an **event queue** (`eq_setup`); hello posts 100 events via `eq_post` — init consumes them with plain loads (no syscall per element), sleeping on the doorbell (`eq_wait`) between batches;
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
| 8  | `eq_setup`      | `rdi`: base, `rsi`: size                    | register an event queue, -> qid                    |
| 9  | `eq_post`       | `rdi`: qid, `rsi`: opcode, `rdx`: buf, `r10`: len | element into the queue + doorbell             |
| 10 | `eq_wait`       | `rdi`: qid                                  | sleep until the doorbell, -> 1                     |
| 11 | `eq_kick`       | `rdi`: qid                                  | hand chunk credit to parked posters                |
| 12 | `initrd_size`   | —                                           | initrd size (RO-mapped at 0x20000000)              |
| 13 | `write`         | `rdi`: buf, `rsi`: len                      | atomic console line (vga+serial)                   |

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
- **initrd** — build.rs compiles every `programs/<name>/main.cpp` against the `shz` runtime into a static ELF and packs them into a ustar archive with a correct checksum; the kernel embeds the archive (page-aligned) and serves images by name on `spawn`.

## Event queue (managarm's HelQueue, adapted)

The queue lives in **user memory**: a ring of chunks with the kernel as producer and the process as consumer. Elements (opcode, sender, payload) are written straight into the chunks through the physmap; the consumer reads them with **plain loads, no syscall per element**, entering the kernel only to sleep on the doorbell (`eq_wait`) or to hand back chunk credit (`eq_kick` after `cons_seq++`). A producer that runs a full lap ahead of the consumer parks until the kick — backpressure without spinning. The data path between doorbells costs zero syscalls, exactly how HelQueue carries async syscall completions in managarm.

## Services in ring 3

- **fsd** — the file service: the kernel shares the initrd read-only into every process address space (0x20000000, page-aligned blob, the same physical frames), fsd parses ustar right in ring 3 and serves file contents to clients over the mailbox;
- **rtcd** — a hardware driver: owns the CMOS ports 0x70/0x71 (ring 3 entered with IOPL=3), waits out the update-in-progress flag, decodes BCD and answers with a time string. A real user-space driver: the kernel knows nothing about the RTC;
- next: keyboard/console drivers as processes fed by the kernel through the event queue.

## Hypervisor begin (Veda-style)

`vmx.rs` lays the foundation: a CPUID check for VT-x, setting `CR4.VMXE`, preparing an aligned VMXON region (revision from IA32_VMX_BASIC), entering VMX root operation via `vmxon` and reading the capability MSRs (pin/procbased/secondary, EPT). Under QEMU TCG VT-x is not exposed — the banner honestly prints "not supported by cpu"; with KVM or on real hardware the kernel enters VMX root and reports the revision. Next, the Veda way: VMCS, vcpu objects, EPT and `GUEST_CREATE`/`VCPU_RUN`-style syscalls — to run a Linux driver VM like Veda does.

### Roadmap

1. Hypervisor: VMCS + vcpu + EPT, GUEST_CREATE/VCPU_RUN syscalls (Veda-style driver VM).
2. Keyboard/console drivers in user space on the event queue; qid rights as a capability instead of raw numbers.
3. Keep peeking: managarm (universe handles, UAR/STAC copies) and Veda (would_cycle detection for capability channels, IOMMU for guest devices).

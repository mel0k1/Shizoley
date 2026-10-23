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
- [x] безопасная работа с памятью пользователя: копии read/write проверяют USER/WRITABLE на каждом листе, kernel-кадры (initrd) помечены BIT_9 и не отдаются
- [x] освобождение физических страниц: bitmap-аллокатор кадров с dealloc; reaper разбирает адресное пространство умершего процесса до кадра
- [x] корректное завершение процессов: выход последнего потока гасит процесс, освобождает кадры, закрывает его очереди и права; kill(14) гасит весь процесс сразу
- [x] обработка ошибок ELF: валидация заголовков, границ, пересечений сегментов, точки входа; при ошибке — откат с возвратом кадров
- [x] граничные случаи IPC: недоставляемое сообщение -> ошибка вместо вечной парковки; kick только у владельца; защита от пересечения очередей в памяти
- [x] клавиатура/консоль как процессы: kbd IRQ -> event queue (eq_listen), consoled рендерит в ring 3
- [x] ps/kill-обвязка: ps(28) снимает живой снепшот registry (pid, потоки, состояние, имя), kill(14) честно отказывает на неизвестном pid; утилиты ps и kill в ring 3, сервис-дискавери через ps_find
- [x] таймер-сервис в ring 3 (timed): ядро даёт сырые тики — ticks(29) читает счётчик PIT, tm_arm(30) ставит one-shot дедлайн (только владелец таймер-синка, выстрел = OP_TICK в очередь), timed мультиплексирует sleep'ы клиентов («SLP»+ms по mailbox -> «WOKE»), shz.tsleep() прячет весь обмен
- [x] очередь-хэндлы как полноценные capabilities: eq_setup возвращает негадуемый 64-битный токен, привязанный к процессу; права (POST/WAIT/KICK/LISTEN/SIGNAL/GRANT) ездят байтами через IPC; конный тест capsdemo: self-grant/forged-token отказы, делегация B->C, would_cycle, revoke-каскад
- [x] would_cycle для грантов (по-ведовски): грант — ребро в лесе дериваций; выдача права назад держателю предка (очередь «дотягивается» сама до себя) отказывается, длинные цепочки — тоже (безопасный ответ); revoke каскадом снимает всё поддерево, владелец может отозвать любой свой сабтри
- [x] гипервизор по Veda-рецепту: VMCS + vcpu + EPT, syscalls GUEST_CREATE/VCPU_RUN/GUEST_DESTROY (unrestricted real-mode guest, vmcall как выход)
- [x] изоляция гостя: UNCONDITIONAL_IO_EXITING — in/out гостя не доходят до реального PIC/PIT/клавиатуры, IO-выход уходит монитору (порт в out[3]); CR4.VMXE под guest/host-маской — гость не может снять VMXE и выпасть из EPT; FEATURE_CONTROL и true-CTLS MSRs проверяются до vmxon
- [x] справедливость планировщика при госте: таймер-выход останавливает гостя и возвращает cpu планировщику, монитор просто перезапускает vcpu_run (vmresume)
- [x] P3: VMX preemption timer (срез гостя ≈ один тик планировщика, exit 52 = reportable) и 2 MiB-листья EPT (коллапс полных физически выровненных регионов, гейт EPT_VPID_CAP[16])
- [x] 64-битный long-mode гость: guest_mode(26) переписывает guest-state под ia-32e вход (PE|PG, PAE, EFER LME|LMA, 64-bit CS), монитор грузит таблицы/код через окно, entry-state (rip/rsp/cr3) передаётся через ctl-половину VcpuExit; gdemo64 — vmcall-гиперсозвон «печать строки» + канал хост→гость через GPR
- [x] zero-copy большие события: eq_zpost(23) отбирает страницы буфера у постера, eq_zmap(24) мапит их владельцу очереди, eq_zdone(25) возвращает кадры в пул; подарок умирает с любой из сторон — кадры всегда возвращаются
- [x] per-op права на qid: грант — произвольное подмножество маски; негативный тест в hello (WAIT без права и подделанный токен — отказы)
- [x] P4: posted interrupts — pin bit 7 + virtual-interrupt delivery (proc2 bit 9) за гейтом апикев; дескриптор PI на гостя (PIR/ON/NV=0xF0), GUEST_PI(27) маршрутизирует вектор хоста, на ext-int выходе ядро постит его в PIR, на входе PIR->vIRR + RVI (с kernel-managed виртуальным EOI — real-mode гость не пишет EOI), CPU доставляет прерывание в гостя без лишних выходов; gdemo считает доставки таймера через IVT-счётчик
- [x] фикс mark_user_ancestors: USER-бит больше не ставится на отсутствующие записи-предки (crate treats raw != 0 as "used" и гуляет по phys 0 как по таблице — первый же мап в свежий pdpt-слот паниковал)

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
│   │   ├── caps.rs          # capability-права на qid (POST/WAIT/KICK/LISTEN)
│   │   └── vmx.rs           # гипервизор: VMXON, VMCS, vcpu, EPT, GUEST_CREATE/VCPU_RUN
│   ├── x86_64-paparaci.json # кастомный target (без red zone)
│   └── .cargo/config.toml
├── userspace/               # окружение на C++20 (freestanding)
│   ├── include/shizoley/    # syscall ABI, io, типы
│   ├── src/                 # рантайм: syscall-обёртки, вывод, _start
│   ├── programs/init/       # init: потоки, ping-pong, сервисы, event queue
│   ├── programs/hello/      # hello: 100 событий через event queue
│   ├── programs/fsd/        # файловый сервис: ustar из RO-маппинга initrd
│   ├── programs/rtcd/       # драйвер часов: CMOS-порты из ring 3 (IOPL=3)
│   ├── programs/timed/      # таймер-сервис: мультиплексор sleep'ов (tm_arm + OP_TICK)
│   ├── programs/ps/         # таблица процессов (ps syscall)
│   ├── programs/kill/       # утилита kill: pid приходит по mailbox
│   ├── programs/sleeper/    # клиент таймер-сервиса, гасится kill'ом посреди сна
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

Полный круг гипервизора проверяется только под KVM: `make run-kvm` (это `-enable-kvm -cpu host` — VMX/EPT пробрасываются в Paparaci, гость L2 печатает «VEDA VMX OK» в serial). Под чистым TCG ядро честно рапортует `vmx unavailable` — этот graceful-путь и проверяет CI. Слой P2: MSR bitmap (чтение TSC проходит напрямую, остальное — выход; RDMSR/WRMSR эмулируются для APIC-base/EFER, остальное — #GP с корректной длиной сбойной инструкции), vAPIC (GPA `0xFEE00000` гостя — virtual-APIC страница за EPT: TPR-shadow и APIC-register virtualization без реального LAPIC), VPID (тегированный TLB на гостя, INVVPID/INVEPT на разрушении/росте), `GUEST_MAP` растит живого гостя (+EPT+окно, INVEPT), а `vcpu_run` **двусторонний**: ядро пишет 20×u64 выходной информации, ring 3 перед следующим входом может переопределить GPR и инжектировать событие (extint/NMI/исключение) — gdemo доказывает это загрузкой GPR и инжекцией int 0x20 с обработчиком в IVT. Изоляция гостя (по итогам сравнения с managarm/Veda, см. `docs/`): порты гостя перехватываются полностью, VMXE под маской, таймер-выход отдаёт cpu планировщику. Подробное сравнение нашей реализации с managarm/Veda: `docs/managarm-veda-vs-paparaci.md`. Фикс v0.9.1: KVM поднимает L1 с `IA32_FEATURE_CONTROL=0`, а его `vmxon` требует `lock|vmx-on` — теперь Paparaci сам лочит feature-control с VMX (как своя прошивка) и подгоняет CR0/CR4 под VMX fixed-MSR до `vmxon` (голый #GP «from ring 0 at vmxon» под `-enable-kvm -cpu host` исчез). P3 (v0.10.0): **VMX preemption timer** — срез гостя ≈ один тик планировщика (калибровка TSC при первом госте, VMCS 0x482E перезаряжается на каждом входе, exit 52 = reportable, resume только через планировщик) — и **2 MiB-листья EPT**: полные физически выровненные регионы коллапсируются в pd-лист 0xB7 (гейт `EPT_VPID_CAP[16]`, выровненный аллокатор кадров, работает на росте через `GUEST_MAP`). v0.11.0: **64-битный long-mode гость** — `guest_mode(26)` переписывает guest-state под ia-32e (PE|PG, PAE, EFER LME|LMA, 64-bit CS, flat-data), монитор гружит identity-таблицы/GDT/64-битный код в гостевую память через окно, а entry-state (rip/rsp/cr3) едет в расширенной ctl-половине VcpuExit (буфер 352 Б); gdemo64 гоняет vmcall «печать строки» и обратный канал хост→гость через GPR (rax=0xBEEF). Плюс **zero-copy события**: `eq_zpost/eq_zmap/eq_zdone` (23-25) — большие данные ездят страницами без единого байта копирования, подарки умирают вместе с любой из сторон; **per-op права** — грант это произвольное подмножество маски, повторный грант заменяет биты. v0.12.0: **posted interrupts** — при доступном apicv (pin bit 7 + virtual-interrupt delivery, оба allowed-1) каждый гость получает дескриптор PI (256-битный PIR + ON + NV=0xF0); `GUEST_PI(27)` маршрутизирует вектор хоста: на ext-int выходе с этим вектором ядро ставит бит в PIR и отдаёт выход монитору, на следующем входе PIR переливается в vIRR с RVI (kernel-managed виртуальный EOI — real-mode гость не пишет EOI), и CPU доставляет прерывание гостю через virtual-interrupt delivery без единого лишнего выхода; без apicv всё честно отказывается (graceful-путь). Попутно закрыт латентный баг `mark_user_ancestors`: USER-бит на отсутствующей записи-предке делал её «используемой» для крейта (raw != 0), и первый же мап в свежий pdpt-слот гулял по phys 0 как по таблице страниц.

Ядро бутится, поднимает heap и MSR, затем планировщик: `main` становится задачей 0, рядом стартует kernel-поток `heartbeat` (доказывает, что вытеснение живёт, пока init блокирован в IPC). Параллельно build.rs кладёт все `programs/*` в ustar-initrd, вшитый в ядро; `spawn_process("init")` находит образ, `elf.rs` раскладывает его в **свежее адресное пространство** (свой CR3) и ставит процесс в очередь. Дальше init в ring 3:

- создаёт **два user-потока** (`thread_create`) — оба работают в одном адресном пространстве, печатают тики и завершаются;
- делает `spawn("hello")` — ядро грузит второй ELF из initrd в отдельный процесс (pid 2) и гоняет с ним ping-pong через mailbox IPC;
- поднимает **сервисы в ring 3**: `fsd` (отдаёт файл `motd` из RO-маппинга initrd по mailbox) и `rtcd` (читает CMOS-часы портами 0x70/0x71 — IOPL=3);
- приносит **таймер-сервис** `timed` (tm_arm/OP_TICK-дедлайн от ядра, мультиплексирование снов клиентов по mailbox), `ps` печатает таблицу процессов, init спит 400 мс через `tsleep`, `kill` гасит припаркованный `sleeper` посреди 5-секундного сна, и вторая таблица `ps` уже без него;
- регистрирует **event queue** (`eq_setup`), hello постит 100 событий через `eq_post` — init вычитывает их обычными load'ами из памяти (без syscall на элемент), между пачками спя в ядре на doorbell (`eq_wait`);
- `exit` последнего потока завершает процесс, жнец печатает коды выходов.

Page fault из ring 3 убивает только провинившуюся задачу — ядро продолжает жить. Клавиатура тоже в ring 3: нажатия ядро постит событиями в очередь consoled, который их рендерит.

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
| 8  | `eq_setup`      | `rdi`: база, `rsi`: размер             | зарегистрировать event queue, -> корневой capability-хэндл (бит 63 всегда 0, чтобы токен не выглядел как ошибка) |
| 9  | `eq_post`       | `rdi`: хэндл, `rsi`: opcode, `rdx`: buf, `r10`: len | элемент в очередь + doorbell (нужен POST) |
| 10 | `eq_wait`       | `rdi`: хэндл                           | спать до doorbell (нужен WAIT), -> 1 события \| 2 alert \| 3 ошибка канала |
| 11 | `eq_kick`       | `rdi`: хэндл                           | отдать chunk-кредит припаркованным постерам (KICK) |
| 12 | `initrd_size`   | —                                      | размер initrd (RO-маппинг на 0x20000000)   |
| 13 | `write`         | `rdi`: buf, `rsi`: len                 | атомарная строка в консоль (vga+serial)    |
| 14 | `kill`          | `rdi`: pid                             | погасить все потоки процесса, -> 0 \| -1   |
| 15 | `guest_create`  | `rdi`: страницы                        | VMCS + EPT + окно RAM, -> handle           |
| 16 | `vcpu_run`      | `rdi`: handle, `rsi`: буфер (40×u64)   | войти в гостя, -> причина vmexit; ядро пишет 20×u64 (reason/rip/qual/gpa/gprs), ring 3 пишет управляющую половину (флаги, инжекция события, gprs) |
| 17 | `guest_destroy` | `rdi`: handle                          | освободить RAM/EPT/VMCS, -> 0              |
| 18 | `eq_listen`     | `rdi`: хэндл, `rsi`: which             | сделать очередь приёмником событий ядра (LISTEN): which 0 = клавиатура, 1 = таймер |
| 19 | `eq_grant`      | `rdi`: хэндл, `rsi`: pid, `rdx`: права | произвести дочерний capability для pid (подмножество своих бит; нужен GRANT), would_cycle-проверка -> новый хэндл |
| 20 | `eq_revoke`     | `rdi`: хэндл                           | каскадно снять поддерево дериваций (держатель токена или его предок) -> сколько хэндлов умерло |
| 21 | `guest_map`     | `rdi`: handle, `rsi`: страницы         | вырастить RAM гостя (+EPT+окно), -> новый размер |
| 22 | `eq_alert`      | `rdi`: хэндл                           | отменить припаркованный eq_wait (SIGNAL), -> 0 |
| 23 | `eq_zpost`      | `rdi`: хэндл, `rsi`: buf, `rdx`: len   | отдать страницы буфера (выровнен, кратен 4К) очереди -> handle подарка |
| 24 | `eq_zmap`       | `rdi`: handle                          | владельцу мапить страницы подарка (RW, NOEXEC) -> va |
| 25 | `eq_zdone`      | `rdi`: handle                          | потребить: отмапить и вернуть кадры в пул -> 0  |
| 26 | `guest_mode`    | `rdi`: handle, `rsi`: mode             | mode 1 = переписать guest-state под long mode -> 0 |
| 27 | `guest_pi`      | `rdi`: handle, `rsi`: вектор           | маршрутизировать вектор хоста в posted-interrupt дескриптор гостя (0 = снять всё), -> 0; требует apicv |
| 28 | `ps`            | `rdi`: buf, `rsi`: max записей         | таблица процессов: 40-байтовые записи (pid, потоки, состояние 0=run/1=dead, имя[16]), -> сколько записано |
| 29 | `ticks`         | —                                      | счётчик PIT-тиков планировщика (100 Гц)    |
| 30 | `timer_arm`     | `rdi`: дедлайн в тиках                 | one-shot дедлайн таймер-синка (только его владелец; 0 = снять); выстрел = OP_TICK с номером тика в очередь, -> 0 |

Права (bitmask): `POST=1`, `WAIT=2`, `KICK=4`, `LISTEN=8`, `SIGNAL=16`, `GRANT=32`. Хэндл — это capability: негадуемый токен, привязанный к процессу-держателю; eq_setup возвращает корневой (все права), eq_grant производит дочерний (подмножество, нужен GRANT), токены передаются по IPC байтами, eq_revoke снимает поддерево. would_cycle отказывает гранту, который вернул бы право держателю предка (очередь «дотянулась» бы сама до себя через собственную цепочку дериваций).

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

1. Подсматриваем дальше: managarm (universe-хэндлы, UAR/STAC-копии, kernel->process данные) и Veda (IOMMU для гостевых устройств). Уже забрано из этого списка: would_cycle-детект (v0.12.0), posted interrupts (v0.12.0), передача handle'ов очередей через capability (v0.12.0, capsdemo).
2. Больше сервисов в ring 3: таймер-сервис, ps/kill-обвязка над pid. Готово: ps/kill-обвязка и таймер-сервис timed (v0.13.0).
3. Гость ещё дальше: REAL hypervisor-драйвер (Veda-стиль drivervm), EPT-префиксы/PAT-память для гостевых устройств.

v0.12.1 — приёмка под posted поймала три ошибки в кодировках полей VMCS, из-за которых ни один гость не мог войти в VM на реальном железе (под TCG это не ловится — там VMX нет вовсе): `VMX_PROC_CTLS2` писался в 0x401E (это TPR-threshold; secondary-ctls живут в 0x4022 — EPT/URG/VID не включались вовсе), `VMX_TPR_THRESHOLD` писался в 0x401C (зарезервированный слот; TPR-threshold = 0x401E), а «virtualize APIC accesses» путалась с битом 5 proc2 (это VPID; настоящий бит — 0, и только под ним легальны APIC-register virtualization и VID); гейт записи VPID-поля переведён на proc2 бит 5, PIN_POSTED гейтится на доступность apicv (без неё пин-бит 7 без VID = отказ VM-entry), vid/reg-virt принудительно снимаются, если access-virt не дожил после clamp. По SDM (Vol 3D, Appendix B) и KVM (SECONDARY_VM_EXEC_CONTROL=0x4022, TPR_THRESHOLD=0x401e).
v0.12.2 — проход по коду поймал класс тихих багов. Самый заметный — busy-spin консоли: ядро ставило EVENT-бит doorbell'а, но никто его не гасил, поэтому после первой посылки eq_wait больше никогда не спал и consoled/init крутили цикл вхолостую на 100% CPU. Теперь eq_wait решает по факту: спит только когда данных нет (prod_seq == cons_seq и открытый чанк пуст), а EVENT-бит стал advisory — потребитель гасит его после дрена, так что lost-wakeup невозможен. Попутно: ipc_recv с сообщением, не влезающим в буфер, возвращает ошибку вместо вечной парковки; eq_setup закрывает гонку overlap-проверки (регистрация и проверка под одним локом); guest_map больше не оставляет валидные записи EPT на откате роста (окно мапится до записи таблиц); коллапс 2 MiB-листьев EPT требует физической непрерывности региона — разрозненный fallback-аллокат с выровненным первым кадром больше не может отдать гостю чужую память; vcpu_run отпускает список гостей до включения прерываний (лок больше не держится через yield планировщика); map_zeroed_range/map_initrd не теряют кадры и не паникуют при исчерпании пула. Баннер ядра теперь печатает CARGO_PKG_VERSION.
v0.13.0 — ps/kill-обвязка и таймер-сервис в ring 3. Ядро: ps(28) снимает живой снепшот registry (40-байтовые записи pid/потоки/состояние/имя, сортировка по pid), kill(14) на неизвестном pid возвращает -1 вместо фальшивого успеха; ticks(29) отдаёт счётчик PIT-тиков; eq_listen(18) получил селектор синка (0 = клавиатура, 1 = таймер); tm_arm(30) ставит one-shot дедлайн в тиках — только владелец таймер-синка, выстрел = OP_TICK с номером тика в очередь (пост из IRQ не блокируется, при неудаче дедлайн перезаряжается — потеря выстрела невозможна), смерть владельца синка разряжает дедлайн. timed: два потока — main собирает запросы клиентов («SLP»+ms по mailbox) и держит таблицу слиперов под спинлоком, драйвер eq_wait'ится на OP_TICK и будит всех, чей дедлайн прошёл, перезаряжая ядро на ближайший будущий; shz.tsleep() находит timed через ps_find (сервис-дискавери через ps!) и прячет весь протокол. Сценарий init: timed/kill/ps/sleeper спавнятся, ps печатает таблицу, init спит 400 мс через timed (sleeper успевает отспать свои 300), потом kill-демон гасит sleeper посреди 5-секундного сна — вторая таблица ps уже без него.

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
- [x] Event queue v2: producer-chained chunks (FULL + next, no lap/FULL ambiguity), error channel in the head (drop/closed/proto codes + count), eq_alert(22) cancels a parked eq_wait (SIGNAL right)
- [x] Ring 3 services: fsd (initrd files over mailbox) and rtcd (CMOS clock via ports 0x70/0x71, IOPL=3)
- [x] initrd shared read-only into every process (0x20000000), parsed in user space
- [x] Safe user-memory handling: read/write copies check USER/WRITABLE on every leaf; kernel frames (initrd) carry BIT_9 and are never handed out
- [x] Freeing physical pages: bitmap frame allocator with dealloc; the reaper tears a dead process's address space down frame by frame
- [x] Correct process termination: the last thread's exit kills the process, frees its frames, closes its queues and grants; kill(14) takes the whole process down at once
- [x] ELF error handling: full header/bounds/segment-overlap/entry validation; on failure the loader rolls back and returns every frame
- [x] IPC edge cases: an undeliverable message fails the syscall instead of parking forever; kick is owner-only; queue memory ranges may not overlap
- [x] Keyboard/console as processes: kbd IRQ -> event queue (eq_listen), consoled renders in ring 3
- [x] ps/kill tooling: ps(28) takes a live registry snapshot (pid, threads, state, name), kill(14) honestly refuses unknown pids; ps and kill utilities in ring 3, service discovery via ps_find
- [x] Ring-3 timer service (timed): the kernel hands out raw ticks — ticks(29) reads the PIT counter, tm_arm(30) arms a one-shot deadline (timer-sink owner only, expiry fires OP_TICK into the queue), timed multiplexes client sleeps ("SLP"+ms over the mailbox -> "WOKE"), shz.tsleep() hides the exchange
- [x] Queue handles as full capabilities: eq_setup returns an unguessable 64-bit token bound to the process; rights (POST/WAIT/KICK/LISTEN/SIGNAL/GRANT) travel as bytes over IPC; the capsdemo round trip: self-grant/forged-token refusals, B->C delegation, would_cycle, revoke cascade
- [x] would_cycle for grants (Veda-style): a grant is an edge in the derivation forest; handing a right back to a holder of an ancestor (the queue reaching itself) is refused, over-long chains too (the safe answer); revoke cascades the whole subtree, the owner can revoke any subtree it spawned
- [x] Hypervisor the Veda way: VMCS + vcpu + EPT, GUEST_CREATE/VCPU_RUN/GUEST_DESTROY syscalls (unrestricted real-mode guest, vmcall as the exit door)
- [x] Guest isolation: UNCONDITIONAL_IO_EXITING — guest in/out never reach the real PIC/PIT/keyboard, IO exits go to the monitor (port in out[3]); CR4.VMXE behind a guest/host mask — a guest cannot drop VMXE and escape the EPT walk; FEATURE_CONTROL and true-CTLS MSRs are checked before vmxon
- [x] Scheduler fairness under a guest: a timer exit stops the guest and hands the cpu back to the scheduler, the monitor simply re-enters via vcpu_run (vmresume)
- [x] P2 virtualization: MSR bitmap (TSC pass-through, emulated APIC-base/EFER, #GP with fault length), vAPIC (virtual-APIC page behind EPT at 0xFEE00000, TPR shadow, APIC-register virtualization), VPID (per-guest tag, INVVPID/INVEPT on teardown), GUEST_MAP(21) grows a live guest (EPT + window + INVEPT), two-way VcpuExit (ring 3 loads GPRs and injects extint/NMI/exception back into the guest)
- [x] P3: VMX preemption timer (guest slice ≈ one scheduler tick, exit 52 = reportable) and 2 MiB EPT leaves (full physically aligned regions collapse, gated on EPT_VPID_CAP[16])
- [x] 64-bit long-mode guest: guest_mode(26) rewrites the guest state for ia-32e entry (PE|PG, PAE, EFER LME|LMA, 64-bit CS), the monitor loads tables/code through the window, entry state (rip/rsp/cr3) travels in the extended VcpuExit ctl half; gdemo64 runs a vmcall "print string" hypercall plus a host→guest channel via GPR
- [x] zero-copy big events: eq_zpost(23) takes the poster's pages away, eq_zmap(24) maps them for the queue owner, eq_zdone(25) returns the frames to the pool; a gift dies with either side — frames always come home
- [x] per-op qid rights: a grant is an arbitrary subset of the mask; negative tests in hello (WAIT without the right and a forged token — both denied)
- [x] P4: posted interrupts — pin bit 7 + virtual-interrupt delivery (proc2 bit 9) behind an apicv gate; a per-guest PI descriptor (PIR/ON/NV=0xF0), GUEST_PI(27) routes a host vector, the kernel posts it into PIR on ext-int exits and syncs PIR->vIRR + RVI on entry (with a kernel-managed virtual EOI — real-mode guests never write EOI), the cpu delivers without further exits; gdemo counts timer deliveries through an IVT counter
- [x] mark_user_ancestors fix: the USER bit is no longer set on absent ancestor entries (the crate treats raw != 0 as "used" and walks phys 0 as a page table — the first map into a fresh pdpt slot panicked)

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

The full hypervisor round trip only works under KVM: `make run-kvm` (that is `-enable-kvm -cpu host` — VMX/EPT are passed through to Paparaci, the L2 guest prints "VEDA VMX OK" on serial). Under plain TCG the kernel honestly reports `vmx unavailable` — that graceful path is what CI checks. P2 hypervisor layer: MSR bitmap (TSC read passes through, everything else exits — RDMSR/WRMSR are emulated for APIC-base/EFER, unknown MSR gets a #GP with the correct fault length), vAPIC (guest GPA `0xFEE00000` is a virtual-APIC page behind EPT: TPR-shadow and APIC-register virtualization serve mmio without a real LAPIC), VPID (tagged TLB per guest, all-context INVVPID/INVEPT on destroy/map), `GUEST_MAP` grows a live guest (+EPT+window, INVEPT after), and `vcpu_run` is **two-way**: the kernel writes 20×u64 of exit info, ring 3 writes back GPR overrides and an event injection (extint/NMI/exception) for the next entry — gdemo proves it by loading a GPR and injecting int 0x20 with a real-mode IVT handler. A detailed comparison of our implementation against managarm/Veda: `docs/managarm-veda-vs-paparaci.md`. v0.9.1 fix: KVM boots an L1 with `IA32_FEATURE_CONTROL=0` while its `vmxon` demands `lock|vmx-on` — Paparaci now locks feature-control itself (acting as its own firmware) and aligns CR0/CR4 with the VMX fixed MSRs before `vmxon` (the bare "gpf from ring 0 at vmxon" under `-enable-kvm -cpu host` is gone). P3 (v0.10.0): **VMX preemption timer** — the guest slice is ~one scheduler tick (TSC calibrated at the first guest, VMCS 0x482E reloaded on every entry, exit 52 = reportable, resume only through the scheduler) and **2 MiB EPT leaves** — full, physically aligned regions collapse into a 0xB7 pd leaf (gated on `EPT_VPID_CAP[16]`, aligned frame allocator, applies to `GUEST_MAP` growth too). v0.11.0: a **64-bit long-mode guest** — `guest_mode(26)` rewrites the guest state for ia-32e (PE|PG, PAE, EFER LME|LMA, 64-bit CS, flat data), the monitor loads identity tables/GDT/64-bit code into guest memory through its window, and the entry state (rip/rsp/cr3) travels in the extended VcpuExit ctl half (352-byte buffer); gdemo64 runs a vmcall "print string" hypercall and a host→guest channel via GPR (rax=0xBEEF). Plus **zero-copy events**: `eq_zpost/eq_zmap/eq_zdone` (23-25) — big data travels as pages without a single byte copied, gifts die with either side; **per-op rights** — a grant is an arbitrary subset of the mask, a repeated grant replaces the bits. A latent `mark_user_ancestors` bug got fixed along the way: a USER bit on an absent ancestor entry made it "used" for the crate (raw != 0), so the first map into a fresh pdpt slot walked phys 0 as a page table.

The kernel boots, sets up the heap and MSRs, then hands control to the scheduler: `main` becomes task 0, and a kernel thread `heartbeat` starts next to it (proving preemption keeps running while init blocks in IPC). Meanwhile build.rs packs every `programs/*` into a ustar initrd embedded into the kernel; `spawn_process("init")` looks the image up, `elf.rs` lays it out into a **fresh address space** (its own CR3) and queues the process. Then init runs in ring 3:

- creates **two user threads** (`thread_create`) — both share the process address space, print ticks and finish;
- calls `spawn("hello")` — the kernel loads the second ELF from the initrd into a separate process (pid 2) and runs a mailbox ping-pong with it;
- brings up **ring 3 services**: `fsd` (serves the `motd` file from the read-only initrd mapping over the mailbox) and `rtcd` (reads the CMOS clock through ports 0x70/0x71 — IOPL=3);
- starts **consoled** — the keyboard/console as a process: it registers its queue as the kernel event sink (`eq_listen`), the kbd IRQ posts `[scancode, ascii]` events, and the boot ping proves the pipe;
- brings up the **ring-3 timer service** `timed` (tm_arm/OP_TICK deadline from the kernel, per-client sleep multiplexing over the mailbox), dumps the process table with `ps`, naps 400 ms through `tsleep`, then `kill` takes the parked `sleeper` down in the middle of its 5 s nap and a second `ps` shows the table without it;
- runs **gdemo** — the Veda-approach smoke: `guest_create` builds VMCS + EPT + a 1 MiB ram window, `guest_map` grows it by one page, a 16-bit blob writes text into its own "VGA" through the EPT, reads the mapped page after a GPR override and takes an injected int 0x20; on machines without VT-x the kernel refuses politely;
- grants hello the `POST` right on its queue (`eq_grant`) and consumes hello's 100 events with plain loads, sleeping on the doorbell between batches;
- when the last thread of a process exits, the reaper tears the process down: address-space frames go back to the bitmap pool, its queues close, its capability grants vanish.

A page fault (or #GP/#UD) from ring 3 kills only the offending process — the kernel lives on. The keyboard no longer echoes in the kernel: keys are events that consoled renders.

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
| 10 | `eq_wait`       | `rdi`: qid                                  | sleep until the doorbell, -> 1 events \| 2 alert \| 3 channel error |
| 11 | `eq_kick`       | `rdi`: qid                                  | hand chunk credit to parked posters                |
| 12 | `initrd_size`   | —                                           | initrd size (RO-mapped at 0x20000000)              |
| 13 | `write`         | `rdi`: buf, `rsi`: len                      | atomic console line (vga+serial)                   |
| 14 | `kill`          | `rdi`: pid                                  | end every thread of the process, -> 0 \| -1        |
| 15 | `guest_create`  | `rdi`: pages                                | VMCS + EPT + ram window, -> handle                 |
| 16 | `vcpu_run`      | `rdi`: handle, `rsi`: buffer (40×u64)       | enter the guest, -> vmexit reason; the kernel writes 20×u64 (reason/rip/qual/gpa/gprs), ring 3 writes the control half (flags, event injection, gprs) |
| 17 | `guest_destroy` | `rdi`: handle                               | free ram/ept/vmcs, -> 0                            |
| 18 | `eq_listen`     | `rdi`: handle, `rsi`: which                 | make the queue a kernel event sink (LISTEN): which 0 = keyboard, 1 = timer |
| 19 | `eq_grant`      | `rdi`: qid, `rsi`: pid, `rdx`: rights       | the owner hands out qid rights                     |
| 20 | `eq_revoke`     | `rdi`: qid, `rsi`: pid                      | the owner takes rights back                        |
| 21 | `guest_map`     | `rdi`: handle, `rsi`: pages                 | grow guest ram (+EPT+window), -> new size          |
| 22 | `eq_alert`      | `rdi`: qid                                  | cancel a parked eq_wait (SIGNAL), -> 0             |
| 23 | `eq_zpost`      | `rdi`: qid, `rsi`: buf, `rdx`: len          | hand the buffer's pages (aligned, 4K multiple) to the queue -> handle |
| 24 | `eq_zmap`       | `rdi`: handle                               | owner maps the gift (RW, NOEXEC) -> va             |
| 25 | `eq_zdone`      | `rdi`: handle                               | consume: unmap and return the frames -> 0          |
| 26 | `guest_mode`    | `rdi`: handle, `rsi`: mode                  | mode 1 = rewrite the guest state for long mode -> 0 |
| 27 | `guest_pi`      | `rdi`: handle, `rsi`: vector                | route a host irq vector into the guest's posted-interrupt descriptor (0 = clear), -> 0; needs apicv |
| 28 | `ps`            | `rdi`: buf, `rsi`: max entries              | process table: 40-byte records (pid, threads, state 0=run/1=dead, name[16]), -> count written |
| 29 | `ticks`         | —                                           | scheduler PIT tick counter (100 Hz)                |
| 30 | `timer_arm`     | `rdi`: deadline in ticks                    | one-shot deadline for the timer sink (its owner only; 0 = disarm); expiry fires OP_TICK into the queue, -> 0 |

qid rights (bitmask): `POST=1`, `WAIT=2`, `KICK=4`, `LISTEN=8`, `SIGNAL=16`, `GRANT=32`. A handle is a capability: an unguessable token bound to the holding process; eq_setup returns the root (all rights), eq_grant derives a child (a subset, needs GRANT), tokens travel as bytes over IPC, eq_revoke takes the derivation subtree back. would_cycle refuses a grant that would hand the right back to a holder of an ancestor (the queue would reach itself through its own derivation chain).

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

The queue lives in **user memory** (v2): a chain of chunks with the kernel as producer and the process as consumer. Elements (opcode, sender, payload) are written straight into the chunks through the physmap; the producer chains a closed chunk to the next one (FULL + `next`), the consumer follows the chain with **plain loads, no syscall per element**, and returns chunks by zeroing their progress word. The head carries a notify word (`EVENT`/`ALERT`/`ERROR` bits) and an error channel (first error code + count: dropped kernel events, closed queue, producer protocol violations). `eq_alert` (SIGNAL right) cancels a parked `eq_wait` — the waiter wakes with 2 instead of blocking until real events show up. A producer that runs a full lap ahead of the consumer parks until the kick — backpressure without spinning. The data path between doorbells costs zero syscalls, exactly how HelQueue carries async syscall completions in managarm.

## Capabilities (queue handles done right)

A queue id is never exposed: `eq_setup` mints a root **capability handle** — an unguessable 64-bit token (xorshift off the tsc, bit 63 kept clear so a token can never alias the `-1` error return) bound to the creating process. Every eq syscall resolves the token against the caller's row of the handle table first; a token copied by a bystander fails the holder check even with the bits right. Rights travel through a **derivation forest**: `eq_grant` derives a child instance from an existing one (a subset of its bits, delegation needs `GRANT`), binds it to the target pid and returns the new token — the granter passes those 8 bytes over the mailbox, the kernel never mediates the transfer. Before an edge is created, `would_cycle` walks the parent chain (Veda's semantics, search limit 4096, over-long chains get the safe answer): handing the capability back to a holder of one of its ancestors would let the queue reach itself through its own chain — revocation loops, self-kept objects — so it is refused. `eq_revoke` takes a whole subtree back (the holder of the token or any of its ancestors may call it; a holder keeps its own instance), and a dead process or queue voids every instance pointing at it. capsdemo/capsuser stage the whole round trip on every boot: self-grant refusal, forged-token refusal, delegation B→C, a delegated post, the refused grant-back, the revoke cascade (2 handles die) and a fresh re-grant.

## Services in ring 3

- **fsd** — the file service: the kernel shares the initrd read-only into every process address space (0x20000000, page-aligned blob, the same physical frames), fsd parses ustar right in ring 3 and serves file contents to clients over the mailbox;
- **rtcd** — a hardware driver: owns the CMOS ports 0x70/0x71 (ring 3 entered with IOPL=3), waits out the update-in-progress flag, decodes BCD and answers with a time string. A real user-space driver: the kernel knows nothing about the RTC;
- the keyboard and console already live in a ring-3 process fed by the kernel through the event queue.

## Hypervisor begin (Veda-style)

`vmx.rs` is the Veda-style heart: after `vmxon` the kernel arms `GUEST_CREATE(15)`/`VCPU_RUN(16)`/`GUEST_MAP(21)`/`GUEST_DESTROY(17)`. `guest_create` builds a VMCS (unrestricted real-mode entry, external-int/NMI exiting, #UD/#GP interception), a 4-level EPT shadowing freshly allocated guest ram plus a virtual-APIC page at GPA `0xFEE00000`, and maps that ram into the creator's address space at `0x30000000` so ring 3 can load images and read results. Controls: MSR bitmap (TSC read passes through, other MSR access exits; RDMSR/WRMSR are emulated for APIC-base/EFER, anything else takes a #GP with the faulting-instruction length so guest handlers can skip it), VPID (unique per guest, INVVPID/INVEPT all-context on teardown), TPR shadow + APIC-register virtualization behind capability checks. `vcpu_run` launches/resumes the vcpu on the task's kernel stack; the exit stub saves guest GPRs, EOIs host interrupts that were acked by the cpu, and returns reportable exits (vmcall — rip advanced, hlt, io, ept violation, apic access, exceptions) to ring 3 with 20×u64 of exit info; before the next entry ring 3 may **override GPRs and inject an event** (extint/NMI/exception with error code and length) — the two-way exit. `guest_map` grows a live guest (new frames + EPT leaves + window, INVEPT after); `guest_destroy` unmaps the window and returns every frame to the pool. Guest isolation (found by the managarm/Veda comparison, see `docs/`): all guest `in`/`out` exits unconditionally (the real PIC/PIT/keyboard are unreachable), `CR4.VMXE` sits behind a guest/host mask with the read shadow set, and `enter_root` validates `IA32_FEATURE_CONTROL` and gates the true-CTLS MSRs on `IA32_VMX_BASIC[55]`. A timer exit stops the guest instead of resuming it in vmx-root, so the scheduler stays fair. Under QEMU TCG VT-x is not exposed — the demo honestly reports "vmx unavailable"; with `-enable-kvm -cpu host` the full round trip works.

### Roadmap

1. Keep peeking: managarm (universe handles, UAR/STAC copies, kernel->process data transfer) and Veda (IOMMU for guest devices). Done from this list: would_cycle for capability channels (v0.12.0), posted interrupts (v0.12.0).
2. More ring 3 services: a timer service, ps/kill tooling over pids. Done from this list: queue handles passed around as capabilities (v0.12.0, capsdemo), ps/kill tooling and the timed timer service (v0.13.0).
3. Guests even farther: a real driver VM (Veda-style), EPT/PAT memory types for guest devices, posted interrupts for a real device-assignment path (the P4 plumbing is in place).

v0.12.1 — the posted-interrupt audit caught three VMCS field-encoding bugs that would have kept every guest out of VM entry on real hardware (invisible under TCG, which has no VMX): `VMX_PROC_CTLS2` was written to 0x401E (that is the TPR-threshold slot; secondary controls live at 0x4022 — EPT/URG/VID were never enabled), `VMX_TPR_THRESHOLD` was written to 0x401C (a reserved slot; TPR-threshold is 0x401E), and "virtualize APIC accesses" was confused with proc2 bit 5 (that is VPID; the real bit is 0, and APIC-register virtualization plus VID are only legal under it). The VPID field write is now gated on proc2 bit 5, PIN_POSTED is gated on apicv availability (pin bit 7 without VID = VM-entry refusal), and vid/reg-virt are force-cleared when access-virt does not survive the capability clamp. Per SDM Vol 3D Appendix B and KVM (SECONDARY_VM_EXEC_CONTROL=0x4022, TPR_THRESHOLD=0x401e).
v0.12.2 — a code pass caught a class of silent bugs. The most visible: console busy-spin — the kernel set the doorbell EVENT bit but nobody ever cleared it, so after the first delivery eq_wait never slept again and consoled/init burned a full core spinning. eq_wait now decides on facts: it sleeps only when no data is pending (prod_seq == cons_seq and the open chunk drained), and the EVENT bit is advisory — the consumer clears it after draining, which makes lost wakeups impossible. Also: ipc_recv with a message too big for the buffer now returns an error instead of parking forever; eq_setup closes the overlap-check race (check and registration under one lock); guest_map no longer leaves valid EPT entries behind on a rolled-back growth (the window maps before any table writes); the 2 MiB EPT-leaf collapse demands physical contiguity — a scattered fallback allocation with an aligned first frame can no longer hand the guest memory it does not own; vcpu_run releases the guest list before re-enabling interrupts (no lock held across a scheduler yield); map_zeroed_range/map_initrd no longer leak frames or panic on pool exhaustion. The kernel banner now prints CARGO_PKG_VERSION.
v0.13.0 — ps/kill tooling and a ring-3 timer service. Kernel: ps(28) takes a live registry snapshot (40-byte pid/threads/state/name records, sorted by pid), kill(14) returns -1 on an unknown pid instead of fake success; ticks(29) exposes the PIT tick counter; eq_listen(18) gained a sink selector (0 = keyboard, 1 = timer); tm_arm(30) arms a one-shot deadline in ticks — timer-sink owner only, expiry fires OP_TICK with the tick count into the sink (the IRQ-context post never blocks, a failed post re-arms so a fire cannot be lost), a dead sink owner disarms. timed runs two threads — main collects client requests ("SLP"+ms over the mailbox) and keeps the sleeper table under a spinlock, the driver eq_waits on OP_TICK and wakes everyone past their deadline, re-arming the kernel on the earliest future one; shz.tsleep() finds timed via ps_find (service discovery through ps!) and hides the whole protocol. The init scenario: timed/kill/ps/sleeper spawn, ps dumps the table, init naps 400 ms through timed (the sleeper's 300 ms nap lands first), then the kill daemon takes the sleeper down in the middle of its 5 s nap — the second table is without it.

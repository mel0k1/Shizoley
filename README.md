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
- [x] universe-хэндлы (v0.14.0): единая capability-таблица на любые объекты ядра (очереди, гости VMX, zero-copy подарки); guest_create возвращает негадуемый токен с правами GRUN/GMAP/GMODE/GPI/GDESTROY, подарки ходят токенами вместо последовательных номеров, cap_grant/cap_revoke работают на любом типе; смерть процесса закрывает его гостей (фикс утечки RAM/EPT/VMCS при kill)
- [x] futex поверх doorbell (v0.14.0): futex_wait(31)/futex_wake(32) — потоки одного процесса спят на слове u32 в своей памяти на той же парковке, что и eq_wait; check-and-park под irq-off (lost-wakeup невозможен), несовпадение значения = выход без сна; fuxdemo гоняет оба пути
- [x] device-assignment путь (v0.15.0): acpi.rs находит RSDP→RSDT/XSDT→DMAR, iommu.rs поднимает VT-d юнит (queued invalidation, root/context-таблицы, pass-through для хостовых устройств или identity-фоллбек) и выдаёт домены — second-level таблицы, зеркалящие EPT гостя; guest_device_attach(33)/detach(34) за правом GDEV; ядерный smoke гоняет e1000 по домену: DMA читает гостевую память (dd), после unmap юнит пишет фолт с адресом и source-id — путь подтверждён под TCG
- [x] UAR/STAC-копии (v0.16.0, managarm-стиль): syscall-пути копируют user-память напрямую через активный user CR3 — stac/clac под SMAP (CPUID-гейт, SMAP/SMEP включаются до VMX), фиксированное окно копий в ядре; #PF из окна = плохой указатель, а не баг ядра: обработчик откатывает RIP на recovery-метку, syscall получает -1; access_ok отсекает kernel-половину и переполнение до копии; uardemo гоняет живую копию, немапленный указатель, kernel-half и переполнение
- [x] EPT/PAT memory types для гостевых устройств (v0.17.0): guest_mmio(35) за правом GMAP мапит memory-BAR приаттаченного PCI-девайса в EPT гостя identity (gpa=hpa) листьями R|W с EMT=UC — гостевой PAT не может сделать BAR кэшируемым (эффективный тип = слабейший из двух), диапазон зеркалится в VT-d домен (device DMA доходит до своего BAR); размер BAR ядро снимает стандартным PCI-probe (pci_mem_bar: IO-BAR/пустые = None, 64-битные поддержаны); недостающие mid-таблицы EPT строит двухпроходный walker (аллокация до первой записи); IA32_PAT программируется явно (WB WT UC- UC WB WT UC- WC); devd — первый driver-vm раунд: BAR как UC, long-mode гость читает STATUS e1000 из MMIO и отдаёт через vmcall
- [x] Memory-объекты — kernel→process data transfer (v0.18.0, managarm-стиль): managarm-набор закрыт полностью — копии (UAR, v0.16.0), одноразовые подарки (eq-zcopy, v0.14.0), теперь персистентные шаримые объекты: shm_create(36) отдаёт кадры под владение ядра (умирают с владельцем), shm_map(37) мапит их в любой процесс с правом MMAP_R (ro) или MMAP_W (rw) — флаги PTE ставит только ядро, ro-держатель физически не может получить запись; листья с BIT_9 как у initrd — teardown процесса не может освободить чужие кадры, close_process снимает маппинги до обхода таблиц и возвращает кадры пула ровно один раз; shmdemo/shmpeer: 16 КБ шарятся без единой копии — доказательство прямо в логе (оба пидa печатают один phys-кадр)
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
│   │   ├── caps.rs          # universe-хэндлы: capability-таблица объектов (eq/guest/gift), гранты, would_cycle
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
│   ├── programs/fuxdemo/    # futex поверх doorbell: два пути wait (мимо и со сном)
│   ├── programs/uardemo/    # uar/stac: плохие user-указатели = -1 syscall, не смерть ядра
│   ├── programs/shmdemo/    # memory-объекты: владелец мапит rw и делегирует ro-право пиру
│   ├── programs/shmpeer/    # peer: мапит те же кадры ro и проверяет паттерн (zero-copy)
│   ├── programs/devd/       # driver-vm раунд: BAR устройства как UC в EPT гостя, MMIO-чтение STATUS из L2
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
| 15 | `guest_create`  | `rdi`: страницы                        | VMCS + EPT + окно RAM, -> корневой capability-хэндл гостя (негадуемый токен) |
| 16 | `vcpu_run`      | `rdi`: handle, `rsi`: буфер (40×u64)   | войти в гостя (нужен GRUN), -> причина vmexit; ядро пишет 20×u64 (reason/rip/qual/gpa/gprs), ring 3 пишет управляющую половину (флаги, инжекция события, gprs) |
| 17 | `guest_destroy` | `rdi`: handle                          | освободить RAM/EPT/VMCS (GDESTROY; все capability гостя умирают с ним), -> 0 |
| 18 | `eq_listen`     | `rdi`: хэндл, `rsi`: which             | сделать очередь приёмником событий ядра (LISTEN): which 0 = клавиатура, 1 = таймер |
| 19 | `cap_grant`     | `rdi`: хэндл, `rsi`: pid, `rdx`: права | произвести дочерний capability для pid на ОБЪЕКТЕ ЛЮБОГО ТИПА (очередь/гость/подарок; подмножество своих бит; нужен GRANT-бит типа), would_cycle-проверка -> новый хэндл |
| 20 | `cap_revoke`    | `rdi`: хэндл                           | каскадно снять поддерево дериваций на объекте любого типа (держатель токена или его предок) -> сколько хэндлов умерло |
| 21 | `guest_map`     | `rdi`: handle, `rsi`: страницы         | вырастить RAM гостя (+EPT+окно, GMAP), -> новый размер |
| 22 | `eq_alert`      | `rdi`: хэндл                           | отменить припаркованный eq_wait (SIGNAL), -> 0 |
| 23 | `eq_zpost`      | `rdi`: хэндл, `rsi`: buf, `rdx`: len   | отдать страницы буфера (выровнен, кратен 4К) очереди -> capability-токен подарка (негадуемый, ZMAP\|ZDONE) |
| 24 | `eq_zmap`       | `rdi`: handle                          | мапить страницы подарка по токену (ZMAP; RW, NOEXEC) -> va |
| 25 | `eq_zdone`      | `rdi`: handle                          | потребить: отмапить и вернуть кадры в пул -> 0  |
| 26 | `guest_mode`    | `rdi`: handle, `rsi`: mode             | mode 1 = переписать guest-state под long mode (GMODE) -> 0 |
| 27 | `guest_pi`      | `rdi`: handle, `rsi`: вектор           | маршрутизировать вектор хоста в posted-interrupt дескриптор гостя (GPI; 0 = снять всё), -> 0; требует apicv |
| 28 | `ps`            | `rdi`: buf, `rsi`: max записей         | таблица процессов: 40-байтовые записи (pid, потоки, состояние 0=run/1=dead, имя[16]), -> сколько записано |
| 29 | `ticks`         | —                                      | счётчик PIT-тиков планировщика (100 Гц)    |
| 30 | `timer_arm`     | `rdi`: дедлайн в тиках                 | one-shot дедлайн таймер-синка (только его владелец; 0 = снять); выстрел = OP_TICK с номером тика в очередь, -> 0 |
| 31 | `futex_wait`    | `rdi`: uaddr, `rsi`: expected          | спать на слове u32, пока *uaddr == expected (парковка = дверок eq), -> 0 уснул \| 1 значение не совпало (не спал) \| -1 |
| 32 | `futex_wake`    | `rdi`: uaddr, `rsi`: n                 | разбудить до n ждущих на слове этого же процесса, -> сколько разбужено |
| 33 | `guest_device_attach` | `rdi`: handle, `rsi`: sid         | положить PCI-устройство (bus<<8 \| dev<<3 \| fn) в IOMMU-домен гостя: домен строится зеркалом EPT (GDEV), -> 0 |
| 34 | `guest_device_detach` | `rdi`: handle, `rsi`: sid         | вернуть устройство: его DMA фолтится, пока оно не приаттачено снова (GDEV), -> 0 |

Права (bitmask, на тип объекта): очередь — `POST=1`, `WAIT=2`, `KICK=4`, `LISTEN=8`, `SIGNAL=16`, `GRANT=32`; гость — `GRUN=1`, `GMAP=2`, `GMODE=4`, `GPI=8`, `GDESTROY=16`, `GGRANT=32`, `GDEV=64`; подарок — `ZMAP=1`, `ZDONE=2`. Хэндл — это capability в единой таблице объектов (universe-хэндлы в духе managarm): негадуемый токен, привязанный к процессу-держателю и указывающий на тип+id объекта ядра; создание объекта возвращает корневой хэндл (все права типа), cap_grant производит дочерний на любом типе (подмножество, нужен GRANT-бит типа), токены передаются по IPC байтами, cap_revoke снимает поддерево. would_cycle отказывает гранту, который вернул бы право держателю предка (объект «дотянулся» бы сам до себя через собственную цепочку дериваций).

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

1. Подсматриваем дальше: managarm (UAR/STAC-копии, kernel->process данные) и Veda (IOMMU для гостевых устройств). Уже забрано из этого списка: would_cycle-детект (v0.12.0), posted interrupts (v0.12.0), передача handle'ов очередей через capability (v0.12.0, capsdemo), universe-хэндлы как обобщение caps на любые объекты ядра (v0.14.0), device-assignment путь — VT-d юнит + домены, зеркалящие EPT гостя, с DMA-доказательством на e1000 (v0.15.0), UAR/STAC-копии с stac/clac и фолт-окном (v0.16.0), EPT/PAT memory types — BAR'ы гостя как UC через EMT + зеркала в VT-d (v0.17.0), memory-объекты — персистентные kernel-owned кадры, шаримые между процессами по capability (v0.18.0).
2. Больше сервисов в ring 3: таймер-сервис, ps/kill-обвязка над pid. Готово: ps/kill-обвязка и таймер-сервис timed (v0.13.0); futex поверх doorbell для синхронизации потоков без сервера (v0.14.0, fuxdemo).
3. Гость ещё дальше: REAL hypervisor-драйвер (Veda-стиль drivervm); posted-interrupt плумбинг ждёт настоящий device-драйвер — IOMMU-половина пути готова (v0.15.0), BAR'ы гостя уже мапятся UC (v0.17.0).

v0.12.1 — приёмка под posted поймала три ошибки в кодировках полей VMCS, из-за которых ни один гость не мог войти в VM на реальном железе (под TCG это не ловится — там VMX нет вовсе): `VMX_PROC_CTLS2` писался в 0x401E (это TPR-threshold; secondary-ctls живут в 0x4022 — EPT/URG/VID не включались вовсе), `VMX_TPR_THRESHOLD` писался в 0x401C (зарезервированный слот; TPR-threshold = 0x401E), а «virtualize APIC accesses» путалась с битом 5 proc2 (это VPID; настоящий бит — 0, и только под ним легальны APIC-register virtualization и VID); гейт записи VPID-поля переведён на proc2 бит 5, PIN_POSTED гейтится на доступность apicv (без неё пин-бит 7 без VID = отказ VM-entry), vid/reg-virt принудительно снимаются, если access-virt не дожил после clamp. По SDM (Vol 3D, Appendix B) и KVM (SECONDARY_VM_EXEC_CONTROL=0x4022, TPR_THRESHOLD=0x401e).
v0.12.2 — проход по коду поймал класс тихих багов. Самый заметный — busy-spin консоли: ядро ставило EVENT-бит doorbell'а, но никто его не гасил, поэтому после первой посылки eq_wait больше никогда не спал и consoled/init крутили цикл вхолостую на 100% CPU. Теперь eq_wait решает по факту: спит только когда данных нет (prod_seq == cons_seq и открытый чанк пуст), а EVENT-бит стал advisory — потребитель гасит его после дрена, так что lost-wakeup невозможен. Попутно: ipc_recv с сообщением, не влезающим в буфер, возвращает ошибку вместо вечной парковки; eq_setup закрывает гонку overlap-проверки (регистрация и проверка под одним локом); guest_map больше не оставляет валидные записи EPT на откате роста (окно мапится до записи таблиц); коллапс 2 MiB-листьев EPT требует физической непрерывности региона — разрозненный fallback-аллокат с выровненным первым кадром больше не может отдать гостю чужую память; vcpu_run отпускает список гостей до включения прерываний (лок больше не держится через yield планировщика); map_zeroed_range/map_initrd не теряют кадры и не паникуют при исчерпании пула. Баннер ядра теперь печатает CARGO_PKG_VERSION.
v0.13.0 — ps/kill-обвязка и таймер-сервис в ring 3. Ядро: ps(28) снимает живой снепшот registry (40-байтовые записи pid/потоки/состояние/имя, сортировка по pid), kill(14) на неизвестном pid возвращает -1 вместо фальшивого успеха; ticks(29) отдаёт счётчик PIT-тиков; eq_listen(18) получил селектор синка (0 = клавиатура, 1 = таймер); tm_arm(30) ставит one-shot дедлайн в тиках — только владелец таймер-синка, выстрел = OP_TICK с номером тика в очередь (пост из IRQ не блокируется, при неудаче дедлайн перезаряжается — потеря выстрела невозможна), смерть владельца синка разряжает дедлайн. timed: два потока — main собирает запросы клиентов («SLP»+ms по mailbox) и держит таблицу слиперов под спинлоком, драйвер eq_wait'ится на OP_TICK и будит всех, чей дедлайн прошёл, перезаряжая ядро на ближайший будущий; shz.tsleep() находит timed через ps_find (сервис-дискавери через ps!) и прячет весь протокол. Сценарий init: timed/kill/ps/sleeper спавнятся, ps печатает таблицу, init спит 400 мс через timed (sleeper успевает отспать свои 300), потом kill-демон гасит sleeper посреди 5-секундного сна — вторая таблица ps уже без него.
v0.14.0 — universe-хэндлы и futex поверх doorbell. Ядро: caps.rs стал единой таблицей объектов (Obj = тип+id: очередь K_EQ, гость VMX K_GUEST, zero-copy подарок K_GIFT) с правами на тип (гость: GRUN/GMAP/GMODE/GPI/GDESTROY/GGRANT; подарок: ZMAP/ZDONE) — grant/revoke/would_cycle работают на любом типе; guest_create теперь возвращает корневой capability-токен (все операции над гостем резолвят его с нужным правом, а не сверяют владение последовательным gid), подарки ходят негадуемыми токенами в OP_ZMSG вместо номеров, eq_zmap/eq_zdone резолвят ZMAP/ZDONE. ФИКС утечки: смерть процесса теперь закрывает его гостей (vmx::close_process до teardown адресного пространства — RAM/EPT/VMCS кадры возвращались в пул только при явном guest_destroy; без этого kill владельца гостя оставлял записи в GUESTS, а teardown освобождал кадры окна второй раз). futex.rs: futex_wait(31)/futex_wake(32) на парковке doorbell (Blocked + switch_away_parked + wake_parked) — check-and-park под irq-off, ключ (pid, uaddr), несовпадение значения = 1 без сна; detach/close вплетены в kill и teardown. fuxdemo: несовпадение = выход без сна, настоящий сон до set+wake, ack через второй futex.

v0.15.0 — device-assignment путь: IOMMU для гостевых устройств (подсмотрено у Veda, сверено с их iommu-crate). Новое: acpi.rs — RSDP-скан (EBDA + BIOS ROM), обход RSDT/XSDT, парсинг DMAR (у QEMU тело заголовка DMAR — 12 байт: HAW, flags и 10 зарезервированных, структуры начинаются с 48, как в linux acpi_table_dmar); memory::map_mmio — окно MMIO в старшей половине (PML4[508], общий поддерево у всех адресных пространств), сохраняющее офсет невыровненной физики (ACPI-таблицы лежат где попало) и мапящее uncached; iommu.rs — VT-d юнит (CAP/ECAP-гейты: queued invalidation обязателен, RWBF — отказ), очередь инвалидаций с wait-дескрипторами и поллингом по FSTS.IQE, root/context-таблицы с copy-on-write на шину (приём Veda), enable = QIE → SRTP → глобальные инвалидации → TE, хостовые устройства идут pass-through (identity-фоллбек для юнитов без PT — мапит всю usable RAM), фолты читаются из fault-records и логируются с source-id и адресом; домены — 4-уровневые second-level таблицы (листья R|W), attach переписывает context-entry устройства с инвалидациями, drop отдаёт устройства «в никуда» и освобождает таблицы. vmx.rs: у гостя появился домен, зеркалящий EPT (строится на первом attach, растёт с guest_map), guest_device_attach(33)/guest_device_detach(34) за правом GDEV, teardown отвязывает устройства до возврата кадров в пул. Ядерный smoke-поток: e1000 включается (PCI command: memory+bus master — без master-бита DMA вообще не доходит до IOMMU), приаттачивается к домену и читает буфер по гостевому адресу (dd ставится), после unmap тот же DMA даёт фолт с точным адресом и source-id — путь перевода доказан под TCG. Сборка переведена на q35 + intel-iommu (TCG-смок и run-kvm).

v0.16.0 — UAR/STAC-копии из managarm-списка. Новое uar.rs: две asm-рутины (paparaci_uar_from/to) на rep movsb — вход под stac (гейт по CPUID.7.EBX[20] и глобалу SMAP_OK: без SMAP инструкция дала бы #UD), фиксированное окно [рутина..end) с recovery-меткой на фолт-путь (clac + «сколько байт не скопировано» из rcx — x86-interrupt ABI хранит все GPR до iret); access_ok до копии отсекает kernel-половину и переполнение адреса без фолта; init() включает SMAP+SMEP до vmx::init — HOST_CR4 в VMCS забирает их с собой. #PF-обработчик: фолт из ring 0 с RIP внутри окна — плохой user-указатель, а не баг ядра: RIP откатывается на recovery-метку, наружу — ненулевой остаток, syscall переводит его в -1. Syscall-пути текущего адресного пространства переведены на прямые копии (write, spawn-имя, ipc send/recv/ps, futex_wait, eq_setup/post, vcpu_run ctl/out); walk через physmap остаётся для чужих CR3 и IRQ-контекста (doorbell/publish, handoff в send). uardemo в ring 3: живая копия работает, немапленный 0xdead0000 даёт [uar] fault … recovered и -1, kernel-half/переполнение режет access_ok.

v0.17.0 — EPT/PAT memory types для гостевых устройств. Ядро: guest_mmio(35) за правом GMAP — memory-BAR девайса из домена гостя мапится в EPT identity (gpa=hpa) листьями R|W с EMT=UC (биты 5:3 = 0): при EPT эффективный тип памяти — слабейший из (гостевой PTE/PAT-тип, EMT), так что BAR нельзя сделать кэшируемым изнутри гостя никак; тот же диапазон зеркалится в VT-d домен — device DMA может ходить в свой же BAR (peer-to-peer). Размер BAR ядро снимает стандартным PCI-probe: dma.rs получил pci_mem_bar() (запись 0xFFFFFFFF в config space, чтение маски, восстановление; IO-BAR'ы и незаполненные BAR'ы = None, 64-битные BAR'ы поддержаны), старый mmio_bar переведён на него. BAR 0xfebc0000 попадает в pdpt-слот LAPIC-страницы (pdpt[3]) и в девственные регионы выше 4G — гостевые mid-таблицы EPT строит двухпроходный walker: read-only обход с точным подсчётом недостающих таблиц (лист BAR ≤ 2M не пересекает границу 2M-региона — максимум 3 кадра), затем аллокация всех кадров разом, затем запись — нехватка памяти не оставляет полусвязанного дерева. memory::init_pat() программирует IA32_PAT явно (WB WT UC- UC WB WT UC- WC — раскладка Linux): PA2 — то, куда ложится NO_CACHE в map_mmio (PCD=1), теперь кэширование MMIO ядра не зависит от firmware-дефолтов, PA7 зарезервирован под WC-framebuffer. devd в ring 3 — первый настоящий driver-vm раунд: e1000 приаттачится к домену гостя, BAR0 (128 КБ) мапится UC, long-mode гость с собственными 4K PTE (P|W|PCD) читает STATUS-регистр напрямую из MMIO и отдаёт значение через vmcall; ept-violation/misconfig ловятся как честные отказы. Под TCG это не исполняется (VMX нет) — devd репортит «vmx unavailable»; реальная проверка — KVM-хост с -cpu host. Попутно пойман латентный баг сборки: SMAP_OK в .bss объявлялся в global_asm без выравнивания — сдвиг раскладки линковки кладёт его на не кратный 8 адрес, и write_volatile(u64) падает на debug-проверке уже на старте ядра; теперь .p2align 3.

v0.18.0 — Memory-объекты: kernel→process data transfer из managarm-списка закрыт полностью. Три механизма переноса данных ядро↔процесс: копии с проверкой указателей (UAR, v0.16.0), одноразовые zero-copy подарки (eq-zcopy, v0.14.0) и теперь персистентные шаримые объекты — то, чего managarm'у и Veda не хватало: кадры владеет объект ядра, а не процесс. memobj.rs: shm_create(36) аллокирует 1–256 кадров и регистрирует объект в caps (новый kind K_MEM, права MMAP_R/MMAP_W/MGRANT — grant/revoke/would_cycle заработали на новом типе без единой строчки в caps.rs, кроме декларации), shm_map(37) мапит кадры в любой процесс: флаги PTE выбирает ядро по правам токена (MMAP_W → RW, иначе RO) — ro-держатель физически не может получить запись, страница не может стать кэшируемой или исполняемой (NO_EXECUTE), va проверяется на попадание в user-половину и коллизии (PageAlreadyMapped = откат установленных страниц, никаких алиасов чужих кадров); листья несут BIT_9 как initrd-маппинги — teardown процесса не может освободить кадры, которые ему не принадлежат, а close_process в reaper'е снимает все маппинги умирающего процесса до обхода его таблиц, объекты умершего владельца разбираются: маппинги живых держателей снимаются, кадры возвращаются пулу ровно один раз, capability на объект глохнут. shmdemo/shmpeer: владелец создаёт 4 страницы, заполняет паттерн rw-маппингом, forged-токен отбивается (негадуемость), ro-ребёнок уходит пиру по mailbox; пир мапит те же кадры ro, проверяет 16384 байта обычными загрузками без единого syscall и unmap'ится до ack — владелец выходит, объект разбирается. Zero-copy доказательство прямо в serial-логе: оба процесса печатают один и тот же phys-кадр (0x316e000), а teardown пира освобождает на 1 кадр больше базы (таблицы маппинга), но не кадры объекта.

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
- [x] Device-assignment path (v0.15.0): acpi.rs finds rsdp→rsdt/xsdt→dmar, iommu.rs arms the vt-d unit (queued invalidation, root/context tables, pass-through for host devices with an identity fallback) and hands out domains — second-level tables mirroring the guest ept; guest_device_attach(33)/detach(34) behind the GDEV right; a kernel smoke drives an e1000 through a domain: dma reads guest memory (dd sets), after an unmap the unit records a fault with the address and source-id — the path is proven under tcg
- [x] UAR/STAC copies (v0.16.0, managarm-style): syscall paths copy user memory directly through the active user cr3 — stac/clac under smap (cpuid-gated, SMAP/SMEP come on before vmx), a fixed copy window in the kernel; a #pf inside the window is a bad pointer, not a kernel bug: the handler rewinds rip to the recovery label, the syscall gets -1; access_ok rejects kernel-half and overflow before the copy; uardemo drives a live copy, an unmapped pointer, kernel-half and overflow
- [x] EPT/PAT memory types for guest devices (v0.17.0): guest_mmio(35) behind the GMAP right maps a memory bar of an attached pci device into the guest ept identity (gpa=hpa) with r|w leaves and emt=uc — the guest pat can never make the bar cacheable (the effective type is the weaker of the two), and the range is mirrored into the vt-d domain (device dma can reach its own bar); the bar size comes from the standard pci probe (pci_mem_bar: io bars and empty slots = none, 64-bit bars supported); missing ept mid tables are built by a two-pass walker (allocation before the first write); ia32_pat is programmed explicitly (wb wt uc- uc wb wt uc- wc); devd is the first driver-vm round: the bar as uc, a long-mode guest reads the e1000 STATUS register from mmio and hands it back over a vmcall
- [x] memory objects — kernel->process data transfer (v0.18.0, managarm-style): the managarm set is fully closed — copies (uar, v0.16.0), one-shot gifts (eq-zcopy, v0.14.0), and now persistent shareable objects: shm_create(36) hands frames to kernel ownership (they die with the owner), shm_map(37) maps them into any process holding MMAP_R (ro) or MMAP_W (rw) — pte flags are chosen by the kernel alone, a ro holder physically cannot gain write access; leaves carry BIT_9 like initrd mappings — a process teardown can never free frames it does not own, close_process removes mappings before the reaper walks the tables and returns frames to the pool exactly once; shmdemo/shmpeer: 16 kib shared without a single copy — the proof is right in the log (both pids print the same phys frame)
- [x] Universe handles (v0.14.0): one capability table for every kernel object kind (queues, VMX guests, zero-copy gifts); guest_create returns an unguessable token carrying GRUN/GMAP/GMODE/GPI/GDESTROY, gifts travel as tokens instead of sequential numbers, cap_grant/cap_revoke work on any kind; a dying process closes its guests (fixes the RAM/EPT/VMCS leak on kill)
- [x] Futex over the doorbell (v0.14.0): futex_wait(31)/futex_wake(32) — threads of one process sleep on a u32 word in their own memory using the same parking as eq_wait; check-and-park under irq-off (lost wakeups impossible), a value mismatch returns without sleeping; fuxdemo runs both paths
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
| 15 | `guest_create`  | `rdi`: pages                                | VMCS + EPT + ram window, -> root capability handle (unguessable token) |
| 16 | `vcpu_run`      | `rdi`: handle, `rsi`: buffer (40×u64)       | enter the guest (GRUN), -> vmexit reason; the kernel writes 20×u64 (reason/rip/qual/gpa/gprs), ring 3 writes the control half (flags, event injection, gprs) |
| 17 | `guest_destroy` | `rdi`: handle                               | free ram/ept/vmcs (GDESTROY; every capability on the guest dies with it), -> 0 |
| 18 | `eq_listen`     | `rdi`: handle, `rsi`: which                 | make the queue a kernel event sink (LISTEN): which 0 = keyboard, 1 = timer |
| 19 | `cap_grant`     | `rdi`: handle, `rsi`: pid, `rdx`: rights    | derive a child capability for pid on ANY object kind (queue/guest/gift; a subset of the caller's bits; the kind's grant bit required), would_cycle-checked -> new handle |
| 20 | `cap_revoke`    | `rdi`: handle                               | cascade the derivation subtree back on any object kind -> handles killed |
| 21 | `guest_map`     | `rdi`: handle, `rsi`: pages                 | grow guest ram (+EPT+window, GMAP), -> new size    |
| 22 | `eq_alert`      | `rdi`: qid                                  | cancel a parked eq_wait (SIGNAL), -> 0             |
| 23 | `eq_zpost`      | `rdi`: qid, `rsi`: buf, `rdx`: len          | hand the buffer's pages (aligned, 4K multiple) to the queue -> gift capability token (unguessable, ZMAP\|ZDONE) |
| 24 | `eq_zmap`       | `rdi`: handle                               | map the gift by its token (ZMAP; RW, NOEXEC) -> va |
| 25 | `eq_zdone`      | `rdi`: handle                               | consume: unmap and return the frames -> 0          |
| 26 | `guest_mode`    | `rdi`: handle, `rsi`: mode                  | mode 1 = rewrite the guest state for long mode (GMODE) -> 0 |
| 27 | `guest_pi`      | `rdi`: handle, `rsi`: vector                | route a host irq vector into the guest's posted-interrupt descriptor (GPI; 0 = clear), -> 0; needs apicv |
| 28 | `ps`            | `rdi`: buf, `rsi`: max entries              | process table: 40-byte records (pid, threads, state 0=run/1=dead, name[16]), -> count written |
| 29 | `ticks`         | —                                           | scheduler PIT tick counter (100 Hz)                |
| 30 | `timer_arm`     | `rdi`: deadline in ticks                    | one-shot deadline for the timer sink (its owner only; 0 = disarm); expiry fires OP_TICK into the queue, -> 0 |
| 31 | `futex_wait`    | `rdi`: uaddr, `rsi`: expected               | sleep on a u32 word while *uaddr == expected (parking = the eq doorbell), -> 0 slept \| 1 mismatch (no sleep) \| -1 |
| 32 | `futex_wake`    | `rdi`: uaddr, `rsi`: n                      | wake up to n waiters of this process on the word, -> how many woke |
| 33 | `guest_device_attach` | `rdi`: handle, `rsi`: sid             | put a pci device (bus<<8 \| dev<<3 \| fn) into the guest's iommu domain: the domain is built as an ept mirror (GDEV), -> 0 |
| 34 | `guest_device_detach` | `rdi`: handle, `rsi`: sid             | give the device back: its dma faults until it is attached elsewhere (GDEV), -> 0 |

Rights per object kind (bitmask): queue — `POST=1`, `WAIT=2`, `KICK=4`, `LISTEN=8`, `SIGNAL=16`, `GRANT=32`; vm guest — `GRUN=1`, `GMAP=2`, `GMODE=4`, `GPI=8`, `GDESTROY=16`, `GGRANT=32`, `GDEV=64`; gift — `ZMAP=1`, `ZDONE=2`. A handle is a capability in one universe handle table (managarm-style): an unguessable token bound to the holding process, naming a typed kernel object (kind + id); creating an object returns the root handle (every right of the kind), cap_grant derives a child on any kind (a subset, the kind's grant bit required), tokens travel as bytes over IPC, cap_revoke takes the derivation subtree back. would_cycle refuses a grant that would hand the right back to a holder of an ancestor (the object would reach itself through its own derivation chain).

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

v0.15.0 — the device-assignment path: an IOMMU for guest devices (peeked at Veda, cross-checked against their iommu crate). New: acpi.rs — an rsdp scan (ebda + bios rom), an rsdt/xsdt walk, dmar parsing (qemu's dmar body header is 12 bytes: haw, flags and 10 reserved — structures start at 48, same as linux's acpi_table_dmar); memory::map_mmio — an mmio window in the upper half (pml4[508], one shared subtree for every address space) that preserves the offset of unaligned physical addresses (acpi tables sit anywhere) and maps uncached; iommu.rs — a vt-d unit (cap/ecap gates: queued invalidation required, rwbf refused), an invalidation queue with wait descriptors and fsts.iqe polling, root/context tables with copy-on-write per bus (Veda's trick), enable = qie → srtp → global invalidations → te, host devices run pass-through (an identity fallback for units without pt — maps all usable ram), faults come out of the fault records logged with source-id and address; domains — 4-level second-level tables (r|w leaves), attach rewrites a device's context entry with invalidations, drop releases devices "to nowhere" and frees the tables. vmx.rs: a guest carries a domain mirroring its ept (built on first attach, grows with guest_map), guest_device_attach(33)/guest_device_detach(34) behind the GDEV right, teardown detaches devices before any frame returns to the pool. A kernel smoke thread: the e1000 is enabled (pci command: memory+bus master — without the master bit dma never reaches the iommu at all), attached to a domain and reads a buffer at its guest address (dd sets), after an unmap the same dma takes a fault with the exact address and source-id — the translation path is proven under tcg. The build now boots q35 + intel-iommu (the tcg smoke and run-kvm both).

v0.16.0 — the UAR/STAC copies from the managarm list. New uar.rs: two asm routines (paparaci_uar_from/to) on rep movsb — entry under stac (gated on cpuid.7.ebx[20] and the SMAP_OK bss flag: without smap the instruction would #ud), a fixed [routine..end) window with a recovery label on the fault path (clac + "bytes left" from rcx — the x86-interrupt abi keeps all gprs until iret); access_ok rejects kernel-half and overflow before any copy; init() turns SMAP+SMEP on before vmx::init so the vmcs HOST_CR4 picks them up. The #pf handler: a ring-0 fault with rip inside the window is a bad user pointer, not a kernel bug — rip rewinds to the recovery label, the leftover count comes back, the syscall turns it into -1. Syscall paths for the current address space now use direct copies (write, spawn name, ipc send/recv/ps, futex_wait, eq_setup/post, vcpu_run ctl/out); the physmap walk stays for remote cr3 and irq context (doorbell/publish, the send handoff). uardemo in ring 3: the live copy works, an unmapped 0xdead0000 gives [uar] fault … recovered and -1, kernel-half/overflow is cut by access_ok.

v0.17.0 — EPT/PAT memory types for guest devices. Kernel: guest_mmio(35) behind the GMAP right — a memory bar of a device in the guest's domain maps into the guest ept identity (gpa=hpa) with r|w leaves and emt=uc (bits 5:3 = 0): under ept the effective memory type is the weaker of (guest pte/pat type, emt), so the bar cannot be made cacheable from inside the guest by any means; the same range is mirrored into the vt-d domain — device dma may reach its own bar (peer-to-peer). The bar size comes from the standard pci probe: dma.rs gained pci_mem_bar() (write 0xffffffff to config space, read the mask back, restore; io bars and empty slots = none, 64-bit bars supported), the old mmio_bar now sits on it. A bar at 0xfebc0000 lands in the lapic page's pdpt slot (pdpt[3]) and in virgin regions above 4g — guest ept mid tables are built by a two-pass walker: a read-only walk that counts exactly the missing tables (a bar leaf of ≤ 2m never crosses a 2m region boundary — at most 3 frames), then one allocation of all frames, then the writes — an allocation failure never leaves a half-wired tree. memory::init_pat() programs ia32_pat explicitly (wb wt uc- uc wb wt uc- wc — the linux layout): pa2 is where map_mmio's NO_CACHE (pcd=1) lands, so kernel mmio caching no longer depends on firmware defaults, and pa7 is reserved for a wc framebuffer. devd in ring 3 — the first real driver-vm round: the e1000 attaches to the guest's domain, bar0 (128 kib) maps as uc, a long-mode guest with its own 4k ptes (p|w|pcd) reads the STATUS register straight from mmio and hands the value back over a vmcall; ept-violation/misconfig surface as honest failures. Under tcg none of this executes (no vmx) — devd reports "vmx unavailable"; the real test is a kvm host with -cpu host. A latent build bug got caught along the way: SMAP_OK was declared in the global_asm .bss without alignment — a linker layout shift can place it at a non-8-aligned address, and write_volatile(u64) trips the debug check right at boot; it is .p2align 3 now.

v0.18.0 — memory objects: the kernel->process data transfer item from the managarm list is fully closed. Three data-transfer mechanisms between the kernel and processes: checked copies (uar, v0.16.0), one-shot zero-copy gifts (eq-zcopy, v0.14.0), and now persistent shareable objects — the piece both managarm and veda have: frames are owned by a kernel object, not by a process. memobj.rs: shm_create(36) allocates 1–256 frames and registers the object in caps (a new kind K_MEM with MMAP_R/MMAP_W/MGRANT rights — grant/revoke/would_cycle started working on the new kind without touching caps.rs beyond the declaration), shm_map(37) maps the frames into any process: the kernel picks the pte flags from the token's rights (MMAP_W → rw, otherwise ro) — a ro holder physically cannot gain write access, and a mapping can never become cacheable or executable (NO_EXECUTE); va is checked against the user half and against collisions (PageAlreadyMapped = roll back the installed pages, no aliasing of foreign frames); leaves carry BIT_9 like initrd mappings — a process teardown can never free frames it does not own, and close_process in the reaper removes all of the dying process's mappings before its tables are walked; objects owned by the dead process are dismantled: live holders' mappings come off, frames return to the pool exactly once, capabilities on the object go dead. shmdemo/shmpeer: the owner creates 4 pages, fills a pattern through the rw mapping, a forged token is refused (unguessable), a ro child travels to the peer over the mailbox; the peer maps the same frames ro, verifies 16384 bytes with plain loads without a single syscall and unmaps before acking — the owner exits and the object is dismantled. The zero-copy proof is right in the serial log: both processes print the same phys frame (0x316e000), and the peer's teardown frees exactly one frame more than the baseline (the mapping's page tables) but none of the object's frames.

## License

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

1. Keep peeking: managarm (UAR/STAC copies, kernel->process data transfer) and Veda (IOMMU for guest devices). Done from this list: would_cycle for capability channels (v0.12.0), posted interrupts (v0.12.0), queue handles as capabilities (v0.12.0, capsdemo), universe handles generalizing caps to any kernel object (v0.14.0), the device-assignment path — a VT-d unit plus domains mirroring the guest EPT, proven by an e1000 dma round (v0.15.0), the UAR/STAC copies with stac/clac and a fault window (v0.16.0), EPT/PAT memory types — guest bars as uc through emt plus vt-d mirrors (v0.17.0), memory objects — persistent kernel-owned frames shared between processes through capabilities (v0.18.0).
2. More ring 3 services: a timer service, ps/kill tooling over pids. Done from this list: queue handles passed around as capabilities (v0.12.0, capsdemo), ps/kill tooling and the timed timer service (v0.13.0), a futex over the doorbell for serverless thread sync (v0.14.0, fuxdemo).
3. Guests even farther: a real driver VM (Veda-style); the posted-interrupt plumbing waits for a real device driver — the IOMMU half of the assignment path is in (v0.15.0), guest bars already map as uc (v0.17.0).

v0.12.1 — the posted-interrupt audit caught three VMCS field-encoding bugs that would have kept every guest out of VM entry on real hardware (invisible under TCG, which has no VMX): `VMX_PROC_CTLS2` was written to 0x401E (that is the TPR-threshold slot; secondary controls live at 0x4022 — EPT/URG/VID were never enabled), `VMX_TPR_THRESHOLD` was written to 0x401C (a reserved slot; TPR-threshold is 0x401E), and "virtualize APIC accesses" was confused with proc2 bit 5 (that is VPID; the real bit is 0, and APIC-register virtualization plus VID are only legal under it). The VPID field write is now gated on proc2 bit 5, PIN_POSTED is gated on apicv availability (pin bit 7 without VID = VM-entry refusal), and vid/reg-virt are force-cleared when access-virt does not survive the capability clamp. Per SDM Vol 3D Appendix B and KVM (SECONDARY_VM_EXEC_CONTROL=0x4022, TPR_THRESHOLD=0x401e).
v0.12.2 — a code pass caught a class of silent bugs. The most visible: console busy-spin — the kernel set the doorbell EVENT bit but nobody ever cleared it, so after the first delivery eq_wait never slept again and consoled/init burned a full core spinning. eq_wait now decides on facts: it sleeps only when no data is pending (prod_seq == cons_seq and the open chunk drained), and the EVENT bit is advisory — the consumer clears it after draining, which makes lost wakeups impossible. Also: ipc_recv with a message too big for the buffer now returns an error instead of parking forever; eq_setup closes the overlap-check race (check and registration under one lock); guest_map no longer leaves valid EPT entries behind on a rolled-back growth (the window maps before any table writes); the 2 MiB EPT-leaf collapse demands physical contiguity — a scattered fallback allocation with an aligned first frame can no longer hand the guest memory it does not own; vcpu_run releases the guest list before re-enabling interrupts (no lock held across a scheduler yield); map_zeroed_range/map_initrd no longer leak frames or panic on pool exhaustion. The kernel banner now prints CARGO_PKG_VERSION.
v0.13.0 — ps/kill tooling and a ring-3 timer service. Kernel: ps(28) takes a live registry snapshot (40-byte pid/threads/state/name records, sorted by pid), kill(14) returns -1 on an unknown pid instead of fake success; ticks(29) exposes the PIT tick counter; eq_listen(18) gained a sink selector (0 = keyboard, 1 = timer); tm_arm(30) arms a one-shot deadline in ticks — timer-sink owner only, expiry fires OP_TICK with the tick count into the sink (the IRQ-context post never blocks, a failed post re-arms so a fire cannot be lost), a dead sink owner disarms. timed runs two threads — main collects client requests ("SLP"+ms over the mailbox) and keeps the sleeper table under a spinlock, the driver eq_waits on OP_TICK and wakes everyone past their deadline, re-arming the kernel on the earliest future one; shz.tsleep() finds timed via ps_find (service discovery through ps!) and hides the whole protocol. The init scenario: timed/kill/ps/sleeper spawn, ps dumps the table, init naps 400 ms through timed (the sleeper's 300 ms nap lands first), then the kill daemon takes the sleeper down in the middle of its 5 s nap — the second table is without it.
v0.14.0 — universe handles and a futex over the doorbell. Kernel: caps.rs became one object table (Obj = kind+id: queue K_EQ, VMX guest K_GUEST, zero-copy gift K_GIFT) with per-kind rights (guest: GRUN/GMAP/GMODE/GPI/GDESTROY/GGRANT; gift: ZMAP/ZDONE) — grant/revoke/would_cycle now work on any kind; guest_create returns a root capability token (every guest operation resolves it with the needed right instead of matching a sequential gid against the owner pid), gifts travel as unguessable tokens inside OP_ZMSG instead of numbers, eq_zmap/eq_zdone resolve ZMAP/ZDONE. Leak fix: a dying process now closes its guests (vmx::close_process runs before the address-space teardown — RAM/EPT/VMCS frames only came back to the pool on an explicit guest_destroy; killing a guest owner left the GUESTS entry behind and the teardown freed the window frames a second time). futex.rs: futex_wait(31)/futex_wake(32) on the doorbell parking machinery (Blocked + switch_away_parked + wake_parked) — check-and-park under irq-off, key (pid, uaddr), a value mismatch returns 1 without sleeping; detach/close are wired into kill and teardown. fuxdemo proves the mismatch fast path, a real park until set+wake, and an ack handshake over a second futex.

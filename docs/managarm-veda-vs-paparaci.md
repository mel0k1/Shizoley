# Подглядывание: managarm и Veda против Paparaci

Дата: 2026-10-22 · база: `main` @ v0.7.0 (`575036e`); P0/P1 из бэклога закрыты в v0.8.0.
Предмет: (1) передача данных «ядро → процесс» в managarm против нашего `eq.rs`;
(2) VMCS/EPT в гипервизоре Veda против нашего `vmx.rs`. Всё сверено по коду
обоих проектов, ссылки `файл:строка` — на их деревья на момент клонирования.

---

## 1. managarm (thor): как ядро отдаёт данные процессу

### 1.1 Очередь из чанков, а не кольцо

Процесс создаёт очередь `helCreateQueue` (#89, `kernel/thor/generic/hel.cpp:446`)
и **сам мапит её** через `helMapMemory` от handle'а очереди — ядро и процесс
работают с одними физическими страницами. Раскладка (`hel/include/hel.h:749-900`,
зеркало в `kernel/thor/generic/thor-internal/ipc-queue.hpp:18-59`):

| Структура | Поля | Комментарий |
|---|---|---|
| `HelQueue` (64 B) | `userNotify`, `kernelNotify`, `cqFirst`, `sqFirst` | два futex-совместимых слова-статуса + индексы первых чанков |
| `HelChunk` | `next`, `progressFutex`, `buffer[]` | заголовок 8 B, чанк выровнен на 64 B |
| `HelElement` (16 B) | `length`, `opcode`, `context` | payload идёт следом, секции выровнены на 8 B |

Две стороны: **SQ** (запросы процесса: «выполни мне MapMemory/ExchangeMsgs/…»)
и **CQ** (completion'ы ядра). Дефолт userspace-обёртки — 8 CQ + 8 SQ чанков
по 4096 B. Статусы: `kHelProgressMask` (валидные байты), `kHelProgressDone`,
`kHelProgressFull`, `kHelNextPresent`.

### 1.2 Главный приём: ядро вообще не разыменовывает user-указатели

Ядро **мапит страницы очереди себе** (`ImmediateWindow`,
`kernel/thor/generic/memory-view.cpp:893-908`) и пишет completion через обычный
`memcpy` по *своему* виртуальному адресу (`ipc-queue.cpp:167-186`), затем
публикует прогресс release-записью в `progressFutex` и атомарным OR-ом
`kUserNotifyCqProgress` в `userNotify`. User-указатели участвуют только в
обработке syscall-аргументов — и там три слоя защиты (`hel.cpp:55-74`):

1. переполнение адреса (`frg::safe_int`) + запрет higher-half;
2. SMAP: `stac`/`clac` вокруг копирования (`arch/x86/cpu.cpp:338`);
3. UAR-протокол (User Access Recovery): bad pointer → fault перехватывается
   (`traps.cpp:6-27`), IP откатывается, syscall возвращает `kHelErrFault`,
   а не роняет ядро.

**Это и есть «красивое решение» managarm**: данные ядра→процесс идут через
общие страницы, но пишет в них только ядро (в CQ), и пишет по своему
маппингу — ни одного доверенного user-указателя в пути.

### 1.3 Уведомления и блокировка

- Ядро: атомарный OR флагов в `userNotify` (release) + подъём внутреннего
  события; юзер чистит биты атомарным AND (слова специально futex-совместимы,
  `hel.h:851-859`).
- Процесс либо поллит `helDriveQueue(#105)`, либо спит **внутри syscall**
  с маской событий (`kHelDriveWait`, `hel.cpp:490-501`) — просыпается по маске.
- Отмена ожидания: `helAlertQueue` (#106) ставит `kUserNotifyAlert` — его зовёт
  другой поток через `Dispatcher::RunQueue::wakeup()`.
- Нарушение протокола очереди (подменённые юзером `cqFirst`/`next`, битые
  длины) → `kUserNotifyError` и отказ, ядро не падает и не спамит
  (`ipc-queue.cpp:69-74, 224-236`).

### 1.4 Большие данные и zero-copy

- Инлайн — сколько влезло в чанк (`validSize: 16+len <= chunkSize`);
  для RPC `RecvInline` жёстко 128 B (`hel.cpp:3397-3402`).
- Стрим > 4096 B — «flow»-протокол: ядровые staging-буферы 4 K × 2
  (double buffering), копирование порциями с ack-пакетами (`hel.cpp:3586-3700`).
- Настоящий zero-copy **не через очередь**: ядро/сосед передают handle
  memory-object, и получатель мапит общие страницы себе (`kHelSubmitMapMemory`);
  для устройств — DMA-пространства + IOMMU.

### 1.5 Сверка с нашим `eq.rs` — что уже есть и что взять

| Механизм | managarm | Paparaci (`eq.rs`) | Вердикт |
|---|---|---|---|
| Общая память очереди | ImmediateMemory + self-map | `setup(base,size)` в user-памяти, ядро пишет через проверенные user-страницы | у нас уже безопасно (Phase 1), приём managarm'а — «свой kernel-алиас» — запасной вариант |
| Права на очередь | per-op: map→Read, drive→Invoke\|Wait, alert→Signal | caps: POST/WAIT/KICK/LISTEN, owner-only grant/revoke | сопоставимо; слить в capability-qid план |
| Уведомление | OR/AND futex-слова + block-in-syscall с маской | doorbell + park-списки, `wait(qid)` | эквивалент по смыслу |
| Ошибки/отмена | `kUserNotifyError`, `kHelAlertQueue` | `-1` из syscall, wake `-1` при `close_process` | взять alert-бит и отдельный error-канал |
| Наполнение | чанки SQ/CQ, O(1) рециклинг | одно кольцо + lap/FULL-логика | у нас на этой логике уже был баг (`420e596`); чанковая v2 снимает класс проблемы |
| Большие данные | инлайн ≤ чанк; zero-copy через handle'ы | payload ≤ 2048 B инлайн, zero-copy нет | zero-copy через общие кадры + handle в событии — в бэклог |

---

## 2. Veda: гипервизор

Расположение: `implementation/kernel/src/architectures/amd64/hv/`
(`vmx.rs` 528 строк, `vcpu.rs` 1191, `ept.rs` 113) + `services/drivervm`
(монитор) + `libraries/abi` (syscall-номера).

### 2.1 Модель

Гость — немодифицированный Linux (bzImage). Процесс-монитор `drivervm` создаёт
`Guest` из VMO (гостевая физическая память) и `Vcpu`, затем крутит
**блокирующий** `VCPU_RUN(94)` на своём потоке: ядро входит в гостя, а из
«медленных» выходов (vmcall, IO, EPT-violation) возвращается в монитор с
структурой `VcpuExit`; монитор отвечает (патчит `data[]`) и зовёт `VCPU_RUN`
снова. На время гостя ядро отпускает BKL; в круге цикла — проверка
`need_resched` (планировщик может вытеснить поток с vcpu). VMCS при каждом
выходе из run-цикла `vmclear`-ится, FPU/6 MSR свапятся — vcpu мигрирует между
CPU (с `invvpid`/`invept`).

### 2.2 Включение VMX и подстройка controls

`caps()` — один раз на систему (`vmx.rs:305-401`): CPUID.VMX,
`IA32_FEATURE_CONTROL` (locked и без VMX → отказ), `IA32_VMX_BASIC`
(VMCS ≤ страницы, memtype WB, **бит 55 / true-CTLS обязателен**),
`IA32_VMX_EPT_VPID_CAP`, fixed-биты CR0/CR4. VMXON — per-CPU лениво, регион с
revision ID из `IA32_VMX_BASIC`.

Подстройка (`vmx.rs:294-298`):

```rust
fn controls(msr: u32, required: u32, optional: u32) -> Option<u32> {
    let caps = rdmsr(msr);
    let (must, may) = (caps as u32, (caps >> 32) as u32);
    (required & !may == 0).then_some((required | (optional & may) | must) & may)
}
```

Требуемый бит, которого CPU не умеет, → `None` → внятная ошибка, а не
vmfail на первом `vmlaunch`.

### 2.3 VMCS (их значения)

- **controls**: pin = EXT-INT | NMI-exit | VIRTUAL_NMI | PREEMPTION_TIMER;
  proc = HLT | MWAIT | RDPMC | CR8-load/store | MOV_DR | **UNCONDITIONAL_IO**
  | MSR_BITMAPS | MONITOR | SECONDARY, при этом CR3-load/store exits
  **запрещены** («CR3 accesses cannot run unintercepted»); proc2 = EPT |
  RDTSCP | INVPCID (+VPID по возможности); exit = HOST_AS_SIZE | PAT | EFER |
  DEBUG; entry = IA32E | PAT | EFER | DEBUG.
- **гость сразу 64-bit** (`libraries/hv/src/architectures/amd64/linux.rs:188-215`):
  unrestricted guest не используется; CS AR `0xA09B` (L=1), data `0xC093`,
  CR0 `0x8005_0033`, CR4 PAE, EFER LME|LMA|NXE; identity-таблицы гостя
  с 2 MiB-листьями монитором кладутся в гостевую память.
- **host state**: CPU-инвариантное в `configure()` (`vcpu.rs:643-656`),
  per-CPU перезаписывается в `load()` (`vcpu.rs:507-517`); `HOST_RSP` —
  на стек потока-монитора (пишется asm'ом), `HOST_RIP` = `vk_vmx_exit`.
- MSR bitmap: страница, вся 0xFF (exit на всё), passthrough для 12 MSR
  (SYSENTER_*, PAT, STAR/LSTAR/CSTAR/SFMASK, FS/GS/KERNEL_GS_BASE, TSC_AUX);
  RDMSR/WRMSR вне списка — эмуляция vAPIC (x2APIC MSR) или `#GP`.
- VPID — статический счётчик 1..=0xFFFF; preemption timer армится каждый круг
  из дедлайна vAPIC-таймера гостя.

### 2.4 EPT (`ept.rs`, целиком 113 строк)

4 уровня; промежуточные записи RWX; листья **только 4 KiB** (huge pages нет);
в листе права R|W|X + memtype из кэширования VMO (WB/WC/UC). EPTP:

```rust
self.root | (6 | (3 << 3))   // memtype WB, walk length 4
```

A/D-биты не включаются. `unmap` зануляет лист; вызывающий обязан прогнать
`tlb::shootdown_guest_memory()` = `invept_all()` + IPI по всем CPU (грубо,
но верно). `Drop` освобождает все таблицы (кадры листьев принадлежат VMO).

### 2.5 VMEXIT-диспетчер (`vcpu.rs:805-955`)

~15 категорий: external-int → собрать прерывание и resume; NMI → `int 2` в
хостовый IDT; CPUID → фильтр гипервизора; RDMSR/WRMSR → vAPIC/TSC/белый
список, иначе `#GP`; HLT → парковка потока до kick/таймера; VMCALL / IO /
EPT-violation → **монитору** (`VcpuExit::HYPERCALL/IO/MEMORY`); triple fault →
SHUTDOWN; CR-access с проверками (mov-to-CR0 требует PE+PG, mov-to-CR4 с
сбросом VMXE → `#GP`); MOV_DR → DR-тень; XSETBV против host XCR0; VMX-инструкции
гостя → `#UD`; EPT_MISCONFIG → `FAILED` (без диагностики gpa!).

### 2.6 Syscalls (`libraries/abi/src/lib.rs:335-375`)

| № | Имя | Суть |
|---|---|---|
| 90 | GUEST_CREATE | handle Guest из resource (HYPERVISOR), ≤ 64 vcpu |
| 91/92 | GUEST_MAP / GUEST_UNMAP | инкрементальный EPT: RWX-флаги, каждый unmap → shootdown |
| 93 | VCPU_CREATE | handle Vcpu + начальный `VcpuState` из user-памяти |
| 94 | VCPU_RUN | блокирующий до exit-типа «Monitor»; ответ монитора — через ту же `VcpuExit` |
| 95 | VCPU_INTERRUPT | вектор в vAPIC + kick из любого потока |
| 96 | VCPU_READ_STATE | снимок состояния, только когда vcpu не гонится |
| 97/98 | GUEST_ATTACH_DEVICE / VCPU_BIND_INTERRUPT | IOMMU-домен + line/MSI в vAPIC |

GUEST_DESTROY нет — объект умирает с последним handle (`Drop`).

---

## 3. Сравнение VMCS/EPT: Veda против нашего `vmx.rs`

Наш сердцевина: `kernel/src/vmx.rs` — VMCS-константы SDM, `vmread/vmwrite/
vmptrld/vmclear`, подстройка TRUE-CTLS, `Vcpu` `#[repr(C)]` 320 B + asm
`vmx_launch_asm`/`vmx_exit_stub`, EPT-билдер, `guest_create/vcpu_run/
guest_destroy` за syscalls 15/16/17.

### 3.1 Совпадения (мы уже в точку)

| Аспект | Veda | Paparaci | Вердикт |
|---|---|---|---|
| Формула подстройки controls | `(required \| optional&may \| must) & may` (`vmx.rs:294`) | `allowed1 & (desired \| !allowed0)` (`vmx.rs:432-437`) | эквивалентны |
| VMXON-регион | страница + revision из `IA32_VMX_BASIC` (`vmx.rs:421`) | то же (`vmx.rs:530-541`) | ✓ |
| vmlaunch/vmresume по флагу | `test rsi,rsi; jnz; vmlaunch/vmresume` | `test esi,esi; jnz 1f` (`vmx.rs:212-219`) | ✓ |
| HOST_RSP на отдельный стек | стек потока-монитора | 4 K `ExitStack` гостя (`vmx.rs:918-920`) | ✓ (у нас даже изолированнее) |
| EPTP | `root \| 6 \| (3<<3)` (`ept.rs:47-49`) | `ept_pml4 \| 0x06 \| (3 << 3)` (`vmx.rs:888`) | байт-в-байт |
| Листья EPT | 4 K, RWX+memtype (`ept.rs:79`) | 4 K, `0x37` = RWX+WB (`vmx.rs:575`) | ✓ (huge pages нет у обоих) |
| External-int exiting | да, сбор прерываний | да + `ack interrupt on exit`, EOI в корне (`vmx.rs:796, 320-326`) | у нас честно короче |
| Сохранение GPR гостя | `GuestRegs` 16+CR2 | `Vcpu.gprs[16]` + `out[20]` (`vmx.rs:129-137`) | ✓ |

### 3.2 Осознанные различия

| Аспект | Veda | Paparaci | Комментарий |
|---|---|---|---|
| Гость | немодифицированный Linux, **сразу 64-bit**, unrestricted guest выключен | 16-bit real-mode блоб через **unrestricted guest** (AR `0x9B`/`0x93`, SMBASE `0x30000`, GDTR/IDTR limit `0xFFFF`) | мы умеем вход «BIOS-style», они — «Linux-style»; наши входы шире, их — выше |
| RAM гостя | VMO + инкрементальный GUEST_MAP/UNMAP, per-leaf memtype | статический линза: кадры выделяются при create, EPT строится целиком, окно `0x30000000` в creator'е | для нашего «загрузил образ и читаешь VGA» — достаточно; инкремент нужен вместе с GUEST_MAP |
| Блокировка/планировщик | BKL release на время гостя, `need_resched` в круге | гость крутится до reportable exit; ext-int выходы резюмятся в корне (`dispatch_in_root`) | см. P1 в бэклоге — справедливость |
| Exits | ~15 категорий, vAPIC и CPUID-фильтр в ядре | 2 авто (ext-int, MSR→#GP), всё остальное — 160 B отчёта в ring 3 | у нас политика в ring 3 — микроядерно правильнее; vAPIC понадобится с настоящим гостем |
| Жизненный цикл VMCS | vmclear на каждом выходе из цикла, свап FPU/6 MSR, миграция CPU | vmptrld на каждый run, vmclear после setup, single-core один гость за раз | ours соответствует our scheduling |
| Уничтожение | нет syscall — Drop по handle | явный `guest_destroy(17)` с отчётом «N frames freed» | для нашей модели владения наш вариант лучше |
| EPT_MISCONFIG | → FAILED, без gpa | reportable, `out[3] = gpa` (`vmx.rs:336-340`) | у нас диагностика лучше |
| Syscall-интерфейс | 90-98, resource-based, двусторонний `VcpuExit` | 15-17, плоские аргументы, out-буфер read-only | двусторонний контракт (патч GPR перед resume) — в бэклог |

### 3.3 Наши пробелы, найденные сравнением

1. **P0 — IO-порты гостя не перехватываются.** ~~У нас в proc-ctls только
   `HLT exiting | secondary`; `UNCONDITIONAL_IO_EXITING` (bit 24) не ставится,~~
   **закрыто в v0.8.0**: bit 24 включён, IO-выход reportable, порт из
   qualification (bits 31:16) кладётся в `out[3]`; контроль за политикой — в
   ring 3 (микроядерный контракт).
2. **P0 — гость может снять VMXE.** ~~Мы не пишем `CR4_GUEST_HOST_MASK`/
   `CR4_READ_SHADOW`; mov-to-CR4 не перехватывается вовсе.~~ **закрыто в
   v0.8.0**: `CR4_GUEST_HOST_MASK = VMXE`, `CR4_READ_SHADOW = 0x2000`
   (запись в VMXE → выход 28 → reportable, чтение видит VMXE установленным;
   маска/shadow согласованы с guest CR4 полем — VM-entry проверка проходит).
3. **P1 — FEATURE_CONTROL не читается.** **закрыто в v0.8.0**: `enter_root`
   читает 0x3A и отказывается с внятной ошибкой при locked-без-VMX (иначе
   `vmxon` умер бы голым #GP).
4. **P1 — true-CTLS без gate.** **закрыто в v0.8.0**: `TRUE_CTLS` берётся из
   `IA32_VMX_BASIC[55]`; без бита используется raw-четвёрка 0x481–0x484 —
   формула `adjust_ctl` одинакова для обоих.
5. **P1 — справедливость.** ~~Timer-exit из гостя сейчас EOI+resume в корне.~~
   **закрыто в v0.8.0**: таймер-выход (после EOI+тик в `dispatch_in_root`)
   останавливает гостя → reportable exit 1 → syscall-граница даёт
   планировщику точку вытеснения; монитор перезапускает `vcpu_run`
   (vmresume безопасен: ext-int инструкцию не рвёт). Контракт: причина 1 =
   «перезапустите vcpu_run» (в userspace ABI появился `VmxExit`).
6. **P2 — MSR bitmap, vAPIC, preemption timer, VPID, 2 MiB-листья EPT,
   GUEST_MAP/UNMAP + invept, двусторонний VcpuExit** — по мере роста гостя
   от 16-бит блоба к чему-то серьёзному.

---

## 4. Бэклог (сортированный)

**vmx.rs**
- [x] P0: `UNCONDITIONAL_IO_EXITING` + IO-exit → reportable (безопасность) — v0.8.0.
- [x] P0: `CR4_GUEST_HOST_MASK = VMXE`, `CR4_READ_SHADOW = 0x2000` — v0.8.0.
- [x] P1: FEATURE_CONTROL (0x3A) read/lock-check в `enter_root` — v0.8.0.
- [x] P1: gate на `IA32_VMX_BASIC[55]` перед `adjust_ctl` — v0.8.0.
- [x] P1: timer-exit → stop гостя → планировщик (справедливость) — v0.8.0.
- [x] P2: MSR bitmap (все доступы — выход; чтение TSC — passthrough;
      RDMSR/WRMSR эмулируются для APIC-base/EFER, остальное — #GP с длиной
      сбойной инструкции из `VM_EXIT_INSTRUCTION_LEN`) — v0.9.0.
- [x] P2: vAPIC — GPA `0xFEE00000` мапится в EPT на apic-access кадр,
      VMCS: `VIRTUAL_APIC 0x2012` + `APIC_ACCESS 0x2014` + TPR-threshold;
      proc2: virtualize-APIC-accesses + APIC-register-virtualization,
      proc: TPR-shadow (зависимости битов согласованы с adjust_ctl) — v0.9.0.
- [x] P2: VPID — тег = id гостя (никогда не 0), гейт по
      `IA32_VMX_EPT_VPID_CAP[32]`, all-context INVVPID на destroy;
      INVEPT all-context после GUEST_MAP и destroy — v0.9.0.
- [x] P2: GUEST_MAP (syscall 21) — рост живого гостя: кадры + EPT-листья
      (+новые PT при переходе 512-границы) + окно создателя, откат на
      любом сбое — v0.9.0. GUEST_UNMAP — не делался (не просилось).
- [x] P2: двусторонний VcpuExit — буфер 40×u64: ядро пишет 20×u64 выхода,
      ring 3 пишет флаги/инжекцию (raw vm-entry interruption-info + err +
      len с валидацией)/16 GPR; vmcall поглощается (rip advance) — v0.9.0.
      gdemo: загрузка GPR + инжекция int 0x20 с IVT-обработчиком.
- [x] P3: preemption timer — pin-ctl bit 6 (allowed-1 гейт), VMCS 0x482E
      перезаряжается на каждом входе; срез ≈ один тик планировщика
      (калибровка TSC по двум тикам при первом госте, VMX_MISC[4:0] —
      ставка декремента); exit 52 = reportable, never resume in root — v0.10.0.
- [x] P3: 2 MiB-листья EPT — гейт EPT_VPID_CAP[16], `alloc_frames_aligned`
      (атомарный выровненный запуск кадров), коллапс полных физически
      выровненных регионов в pd-лист 0xB7 (PT остаётся выделенным для
      единообразной индексации), работает и на росте через GUEST_MAP —
      v0.10.0. Заодно закрыт латентный баг индексации новых PT в
      guest_map (листья попадали в apic_pd при переходе через 512-границу).
- [ ] P3+: posted interrupts (needs vAPIC story).

**eq.rs / caps**
- [x] P2: error-канал — head+32 error_code (первый код: E_DROP/E_CLOSED/
      E_PROTO), head+36 err_count, notify-бит ERROR; eq_wait -> 3 при
      ошибке без событий — v0.9.0.
- [x] P2: alert — eq_alert(22) с правом SIGNAL (0x10): notify-бит ALERT,
      все припаркованные wait просыпаются с 2; fast-path в wait — v0.9.0.
- [x] P2: чанковая очередь v2 — producer-chain (FULL + next), консьюмер
      идёт по цепочке и возвращает чанки (progress=0); класс багов
      lap/FULL снят — v0.9.0 (заголовок 48 Б, магия "ZHv2").
- [ ] P2: zero-copy больших событий: общие кадры + handle в событии.
- [ ] P2: слить права caps с capability-qid планом (per-op права).

---

## 5. Тестирование: `-enable-kvm -cpu host`

Новый таргет `make run-kvm`:

```bash
qemu-system-x86_64 -enable-kvm -cpu host \
    -drive format=raw,file=kernel/target/x86_64-paparaci/debug/bootimage-paparaci.bin \
    -serial stdio
```

Почему именно так:

- `-enable-kvm` — L0-гипервизор даёт L1 (Paparaci) **настоящий VMX**: KVM
  эмулирует nested VMX, наши VMCS/EPT превращаются у него в shadow-combined
  структуры. Чистый TCG VMX не экспонирует — ядро честно пишет
  `[vmx] probe: not supported by cpu` и гость demo рапортует graceful
  «vmx unavailable» (этот путь и ловит CI-smoke).
- `-cpu host` — пробрасывает флаги хостового CPU: VMX, EPT,
  unrestricted-guest, true-CTLS — всё, что мы проверяем в `init()`.
- Требования: `/dev/kvm` на хосте; если Paparaci запускается внутри VM —
  nested на L0 (`modprobe kvm_intel nested=1`); в BIOS не должен быть
  залочен `IA32_FEATURE_CONTROL` без VMX (см. P1-фикс выше — тогда получим
  внятное сообщение, а не #GP).

Ожидаемый serial-лог полного круга:
`[vmx] on: rev … ept 1 urg 1 — guest_create/vcpu_run armed` →
`[vmx] guest 1 created: …` → `VEDA VMX OK` (гость L2 пишет в `0xB8000`
через EPT) → vmcall как первый reportable exit в ring 3 →
`[vmx] guest 1 destroyed: … frames freed`.

---

## 6. Ссылки

- managarm: `kernel/thor/generic/{ipc-queue.cpp, ipc-queue.hpp, hel.cpp,
  memory-view.cpp, traps.cpp}`, `hel/include/hel.h`, userspace `helix/ipc.hpp`.
- Veda: `implementation/kernel/src/architectures/amd64/hv/{vmx,vcpu,ept}.rs`,
  `implementation/kernel/src/hv/`, `implementation/libraries/hv/src/
  architectures/amd64/linux.rs`, `implementation/services/drivervm/`,
  `implementation/libraries/abi/src/lib.rs`.
- Paparaci: `kernel/src/{vmx.rs, eq.rs, caps.rs, syscall.rs}`,
  userspace `programs/{consoled,gdemo}`.

Клоны для дальнейшего чтения: `/home/z/my-project/refs/{managarm,Veda}`.


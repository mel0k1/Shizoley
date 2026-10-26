#!/bin/bash
# tcg smoke for paparaci: full boot scenario (uar, caps, futex, iommu, guests)
set -u
export LD_LIBRARY_PATH=/home/z/my-project/.tools/usr/lib/x86_64-linux-gnu
QEMU=/home/z/my-project/.tools/bin/qemu-system-x86_64
QLIB=/home/z/my-project/.tools/usr/share/qemu
IMG=/home/z/my-project/shizoley/kernel/target/x86_64-paparaci/debug/bootimage-paparaci.bin
LOG=/home/z/my-project/shizoley/scripts/serial.log
rm -f "$LOG"
timeout 150 "$QEMU" -L "$QLIB" \
  -machine q35 -device intel-iommu,intremap=off \
  -device e1000,netdev=n0 -netdev user,id=n0 \
  -drive format=raw,file="$IMG" \
  -display none -serial file:"$LOG" -no-reboot -no-shutdown
rc=$?
echo "--- qemu exit rc=$rc (124=timeout, expected for hlt loop)"
FAIL=0
ck() { if grep -qF "$1" "$LOG"; then echo "OK   $1"; else echo "MISS $1"; FAIL=1; fi; }
cke() { if grep -qE "$1" "$LOG"; then echo "OK   ($1)"; else echo "MISS ($1)"; FAIL=1; fi; }

cke "\[init\] threads: [0-9]+ [0-9]+"
ck "[t2] tick 2"
ck "[init] pong from 2"
ck "[fsd] served 'motd' (104 bytes) to 1"
cke "\[init\] rtc says: [0-9][0-9]:[0-9][0-9]:[0-9][0-9]"
ck "[eq] first event: 'e0' op 1 from 2"
ck "[eq] 100 events in"
ck "[init] drained 101 events, bye"
ck "ello] wait denied (no WAIT right): per-op caps ok"
cke "\[eq\] zcopy: 8192 bytes via handle [0-9]+: pattern ok"
ck "[eq] zcopy: gift 1 consumed (2 pages freed)"
ck "[gdemo64] vmx unavailable (guest_create refused)"
ck "[init] spawned devd"
ck "[devd] vmx unavailable (guest_create refused)"
ck "[pat] wb wt uc- uc wb wt uc- wc"
ck "process 'init' (1) finished"
ck "[vmx]"
ck "[console] up: kernel sink attached"
ck "[gdemo] vmx unavailable (guest_create refused)"
cke "\[caps\] granted 0x1 on queue [0-9]+ to pid 2"
ck "[eq] wait: alerted (wait canceled)"
cke "\[eq\] queue [0-9]+ closed \(owner 1 gone\)"
ck "ello] forged token denied: capabilities are unguessable"
ck "[capsdemo] all ok"
ck "[caps] would_cycle refused"
ck "[capsuser] post denied after revoke (cascade ok)"
cke "process 'init' \(1\) torn down, [0-9]+ frames freed"
ck "[timed] up: kernel timer sink attached"
cke "\[eq\] kernel event sink timer -> queue [0-9]+ owned by pid [0-9]+"
ck "[sleeper] slept 300ms (measured "
ck "[init] napped (measured "
ck "[timed] fired at tick "
cke "\[ps\] [0-9]+ run [0-9]+ timed"
cke "\[ps\] [0-9]+ run [0-9]+ sleeper"
cke "\[kill\] target [0-9]+ terminated \(by [0-9]+\)"
ck "[init] kill ok"

ck "[futex] mismatch returned 1 (no sleep)"
ck "[futex] woke, word 42"
ck "[futex] all ok"
ck "[init] spawned fuxdemo"

ck "[iommu] unit 0xfed90000: levels 4 aw 2 pt true"
ck "[iommu] unit 0xfed90000: te on (host pass-through)"
ck "[iommu] device 00:02.0 -> domain "
cke "\[nic\] intx: pin [0-9]+ line [0-9]+ -> vector 0x[0-9a-f]+ \(unmasked\)"
ck "[iommu] dma ok: nic read gpa 0x8000 through domain 2 (dd set, via irq"
ck "[iommu] fault: 0x0010 0x8000 read (not readable)"
ck "[iommu] device 00:02.0 out of domain "

ck "[shm] mem 1 created: 4 pages for pid"
ck "[shmdemo] forged token denied"
ck "[shmpeer] verified 16384 bytes (ro map, no copies)"
ck "[shmdemo] share ok (pid "
ck "[shm] mem 1 freed: 4 frames returned"
P1=$(sed -n 's/.*pid [0-9]* maps mem 1 at 0x10000000 phys 0x\([0-9a-f]*\).*/\1/p' "$LOG" | head -1)
P2=$(sed -n 's/.*pid [0-9]* maps mem 1 at 0x11000000 phys 0x\([0-9a-f]*\).*/\1/p' "$LOG" | head -1)
if [ -n "$P1" ] && [ "$P1" = "$P2" ]; then
  echo "OK   zero-copy share (same phys frame 0x$P1)"
else
  echo "MISS zero-copy share (phys 0x$P1 vs 0x$P2)"; FAIL=1
fi

# v0.21.0: host rx smoke — arp + icmp echo round trip on the intx path
ck "[nic] mac 52:54:00:12:34:56"
ck "[nic] arp reply: 10.0.2.2 at 52:55:0a:00:02:02"
ck "[nic] rx ok: arp + icmp echo round trip (host smoke)"
# v0.22.0: the udp half — dhcp discover -> offer against slirp's server
ck "[nic] udp: dhcp offer for 10.0.2.15"
ck "[nic] udp ok: dhcp discover -> offer round trip"
ck "[nic] down: device released"

exit $FAIL

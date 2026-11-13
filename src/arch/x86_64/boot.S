.section .multiboot2_header, "a"
.align 8
multiboot2_header_start:
.long 0xE85250D6
.long 0
.long multiboot2_header_end - multiboot2_header_start
.long -(0xE85250D6 + 0 + (multiboot2_header_end - multiboot2_header_start))
.short 1
.short 0
.long 12
.long 6
.align 8
.short 0
.short 0
.long 8
multiboot2_header_end:

.section .bss, "aw", @nobits
.align 4096
p4_table:
.skip 4096
p3_table:
.skip 4096
p2_table:
.skip 4096
.align 16
stack_bottom:
.skip 16384
stack_top:

.section .rodata, "a"
.align 8
gdt64:
.quad 0
.quad 0x00AF9A000000FFFF
.quad 0x00CF92000000FFFF
gdt64_pointer:
.short gdt64_pointer - gdt64 - 1
.long gdt64

.section .text, "ax"
.code32
.global _start
_start:
mov esp, offset stack_top
mov esi, ebx
cmp eax, 0x36D76289
jne boot_error
call boot_check_cpuid
call boot_check_long_mode
call boot_setup_page_tables
call boot_enable_paging
lgdt [gdt64_pointer]
ljmp 0x8, offset boot_long_mode

boot_check_cpuid:
pushfd
pop eax
mov ecx, eax
xor eax, 1 << 21
push eax
popfd
pushfd
pop eax
push ecx
popfd
cmp eax, ecx
je boot_error
ret

boot_check_long_mode:
mov eax, 0x80000000
cpuid
cmp eax, 0x80000001
jb boot_error
mov eax, 0x80000001
cpuid
test edx, 1 << 29
jz boot_error
ret

boot_setup_page_tables:
mov edi, offset p4_table
mov ecx, 3072
xor eax, eax
rep stosd

mov eax, offset p3_table
or eax, 3
mov dword ptr [p4_table], eax

mov eax, offset p2_table
or eax, 3
mov dword ptr [p3_table], eax

xor ecx, ecx
boot_map_p2_loop:
mov eax, ecx
shl eax, 21
or eax, 0x83
mov dword ptr [p2_table + ecx*8], eax
inc ecx
cmp ecx, 512
jne boot_map_p2_loop
ret

boot_enable_paging:
mov eax, offset p4_table
mov cr3, eax
mov eax, cr4
or eax, 1 << 5
mov cr4, eax
mov ecx, 0xC0000080
rdmsr
or eax, 1 << 8
wrmsr
mov eax, cr0
or eax, 1 << 31
or eax, 1 << 0
mov cr0, eax
ret

boot_error:
mov dword ptr [0xB8000], 0x4F524F45
mov dword ptr [0xB8004], 0x4F3A4F52
boot_error_halt:
hlt
jmp boot_error_halt

.code64
boot_long_mode:
mov ax, 0x10
mov ss, ax
mov ds, ax
mov es, ax
mov fs, ax
mov gs, ax
lea rsp, [rip + stack_top]
mov edi, esi
call rust_main
boot_halt:
hlt
jmp boot_halt

.section .text
.code64

.global gdt_load
gdt_load:
    lgdt [rdi]
    push 0x08
    lea rax, [rip + 1f]
    push rax
    retfq
1:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax
    mov ax, 0x18
    ltr ax
    ret

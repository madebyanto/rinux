.section .text
.code64

.extern TSS
.extern syscall_dispatch

.global syscall_entry
syscall_entry:
    cld
    mov [rip + SYSCALL_USER_RSP], rsp
    mov rsp, qword ptr [rip + TSS + 4]

    push 0x2B
    push qword ptr [rip + SYSCALL_USER_RSP]
    push r11
    push 0x33
    push rcx

    push rax
    push rbx
    push rcx
    push rdx
    push rsi
    push rdi
    push rbp
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15

    mov rbx, rsp
    mov rdi, rbx
    and rsp, -16
    call syscall_dispatch
    mov [rbx + 112], rax
    mov rsp, rbx

    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rbp
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rbx
    pop rax

    pop rcx
    add rsp, 8
    pop r11
    mov rsp, qword ptr [rsp]
    sysretq

.section .bss
.align 8
SYSCALL_USER_RSP:
    .quad 0

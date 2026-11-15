.section .text
.code64

.global context_switch
context_switch:
    mov [rdi + 0], rsp
    pushfq
    pop rax
    mov [rdi + 8], rax
    mov [rdi + 16], rbx
    mov [rdi + 24], rbp
    mov [rdi + 32], r12
    mov [rdi + 40], r13
    mov [rdi + 48], r14
    mov [rdi + 56], r15

    mov rsp, [rsi + 0]
    mov rbx, [rsi + 16]
    mov rbp, [rsi + 24]
    mov r12, [rsi + 32]
    mov r13, [rsi + 40]
    mov r14, [rsi + 48]
    mov r15, [rsi + 56]
    mov rax, [rsi + 8]
    push rax
    mov eax, 0
    popfq
    ret

.global task_start
task_start:
    sti
    call rbx
    cli
    call task_finished
    hlt

.global fork_child_start
fork_child_start:
    mov rsp, r12
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

.global enter_user_mode
enter_user_mode:
    mov rax, rdi
    mov rdx, rsi
    push 0x2B
    push rdx
    push 0x202
    push 0x33
    push rax
    cld
    xor eax, eax
    xor ebx, ebx
    xor ecx, ecx
    xor edx, edx
    xor esi, esi
    xor edi, edi
    xor ebp, ebp
    xor r8d, r8d
    xor r9d, r9d
    xor r10d, r10d
    xor r11d, r11d
    xor r12d, r12d
    xor r13d, r13d
    xor r14d, r14d
    xor r15d, r15d
    iretq

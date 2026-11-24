class Win32Abi:
    name = "win32"
    wraps_calls = True

    @staticmethod
    def impl_name(ident):
        return f"{ident}_stub"

    def thunk(self, ident, iface, slot, method, sig, argbytes=None, ret=None):
        n = (argbytes or 0) // 4
        k = ident[1:].split("_")[0]
        params = "".join(f", uint32_t a{i}" for i in range(n))
        unused = "".join(f"(void)a{i}; " for i in range(n))
        log = f'shim_unimplemented("{iface}", {slot}, "{method or "?"}");'
        if ret == "struct8" and n >= 1:
            body = f'{log} *(uint64_t *)(uintptr_t)a0 = 0; return a0;'
        elif ret == "call":
            body = f'{log} return shim_callresult_fail();'
        else:
            body = f'{log} return 0;'
        stub = (f'static uint64_t __attribute__((thiscall)) {ident}_stub(void *self{params}) {{ (void)self; {unused}'
                f'{body} }}\n')
        pushes = "".join(f'"\\tpushl {8 + 4 * i}(%ebp)\\n"\n    ' for i in reversed(range(n)))
        ret = f"ret ${4 * n}" if n else "ret"
        asm = (f'void {ident}(void);\n'
               f'__asm__(".text\\n.globl _{ident}\\n_{ident}:\\n"\n'
               f'    "\\tpushl %ebp\\n\\tmovl %esp, %ebp\\n"\n'
               f'    {pushes}"\\tcall *_impl_{k}+{4 * slot}\\n"\n'
               f'    "\\tcmpl %ebp, %esp\\n\\tjne 1f\\n"\n'
               f'    "2:\\tmovl %ebp, %esp\\n\\tpopl %ebp\\n\\t{ret}\\n"\n'
               f'    "1:\\tmovl %esp, %ecx\\n\\tsubl %ebp, %ecx\\n\\tmovl %ebp, %esp\\n"\n'
               f'    "\\tpushl %eax\\n\\tpushl %edx\\n\\tpushl %ecx\\n\\tpushl ${slot}\\n\\tpushl ${k}\\n"\n'
               f'    "\\tcall _shim_arity_mismatch\\n\\taddl $12, %esp\\n\\tpopl %edx\\n\\tpopl %eax\\n\\tjmp 2b\\n");\n')
        return stub + asm

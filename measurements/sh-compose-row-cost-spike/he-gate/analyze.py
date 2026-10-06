"""Count runtime checks per function and loop in an emitted checked .metal file.

usage: python3 analyze.py   (in the dir holding emit_msl.rs's *_checked.metal)
Prints loop-bound counters, const-index and buffer-length clamps and
naga_div/naga_mod guards per function and loop; analysis.txt is its output.
"""
import re,sys,collections
def analyze(path):
    func=None; depth=0; stack=[]; pend=None
    res=collections.Counter()
    for ln in open(path).read().split('\n'):
        if depth==0:
            m=re.match(r'^(?!struct|constant|#|using|namespace|typedef)[^\s].*?(\w+)\(',ln)
            if m: func=m.group(1)
        m2=re.search(r'uint2 (loop_bound\w*) = ',ln)
        if m2: pend=m2.group(1)
        isloop='while(true) {' in ln
        if isloop: stack.append([pend if pend else 'loop',None])
        ctx=stack[-1][0] if stack else '-'
        k=lambda kind:(func,ctx,kind)
        for mm in re.finditer(r'(\w+)(?:\.inner)?\[metal::min\(unsigned\(.*?\), (\d+)u\)\]',ln):
            res[k('idx_clamp(const) '+mm.group(1))]+=1
        for mm in re.finditer(r'(\w+)\[metal::min\(unsigned\(.*?\), \(_buffer_sizes\.size\d+ - 0 - \d+\) / \d+\)\]',ln):
            res[k('buf_len_clamp '+mm.group(1))]+=1
        if re.search(r'loop_bound\w* -= ',ln): res[k('loop_counter')]+=1
        if not re.match(r'^\S',ln):
            c=len(re.findall(r'naga_(?:div|mod)\(',ln))
            if c: res[k('int_div_guard')]+=c
        depth+=ln.count('{')-ln.count('}')
        if isloop: stack[-1][1]=depth
        while stack and stack[-1][1] is not None and depth<stack[-1][1]: stack.pop()
        if depth==0 and not ln.startswith(' '): pass
    return res
if __name__=='__main__':
    for n in ['sh_compose','animated_direct']:
        print('###',n)
        r=analyze(n+'_checked.metal')
        for (f,c,kind),v in sorted(r.items(),key=lambda x:(str(x[0][0]),str(x[0][1]),x[0][2])): print(f'{str(f):32s} {c:14s} {kind:36s} {v}')

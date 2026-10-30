import csv
rows=list(csv.DictReader(open('decomp.csv')))
for r in rows:
    for k in r:
        if k not in('label','oracleTrip','rectTrip'): r[k]=int(r[k])
def agg(sel,name):
    n=len(sel)
    if not n: return
    m=lambda k: sum(r[k] for r in sel)/n
    tot=lambda k: sum(r[k] for r in sel)
    print(f"{name}: n={n} meanR={m('R'):.1f} E={m('E'):.1f} S={m('S'):.1f} R-E={m('RmE'):.1f} E-S={m('EmS'):.1f} | wR={m('wR'):.0f} wE={m('wE'):.0f} wS={m('wS'):.0f} wR-wE={m('wR')-m('wE'):.0f} wE-wS={m('wE')-m('wS'):.0f} | cell share of looseness={tot('RmE')/max(1,tot('RmE')+tot('EmS')):.2f} gran={tot('EmS')/max(1,tot('RmE')+tot('EmS')):.2f}; weighted looseness share={(tot('wR')-tot('wE'))/max(1,tot('wR')-tot('wS')):.2f}")
agg(rows,'all')
agg([r for r in rows if r['E']>=20],'E>=20')
agg([r for r in rows if r['E']>=20 and r['label'].startswith('hall')],'hall E>=20')
agg([r for r in rows if r['E']>=20 and not r['label'].startswith('hall')],'room2/pass E>=20')
agg([r for r in rows if r['E']>=60],'E>=60')
rs=sorted(r['RmE'] for r in rows); es=sorted(r['EmS'] for r in rows)
print('R-E p50/p95/max',rs[len(rs)//2],rs[int(len(rs)*.95)],rs[-1],' E-S',es[len(es)//2],es[int(len(es)*.95)],es[-1])
print('max oracle steps',max(r['oracleSteps'] for r in rows), 'trips', sum(r['oracleTrip']=='true' for r in rows))
for r in sorted(rows,key=lambda r:-r['EmS'])[:8]: print(r['label'],'R',r['R'],'E',r['E'],'S',r['S'],'R-E',r['RmE'],'E-S',r['EmS'],r['wR'],r['wE'],r['wS'])

import sys,os,struct,collections
SOF={0xC0:'SOF0 baseline',0xC1:'SOF1 ext-seq',0xC2:'SOF2 PROGRESSIVE',0xC3:'SOF3 lossless',
     0xC5:'SOF5',0xC6:'SOF6',0xC7:'SOF7',0xC9:'SOF9 ARITHMETIC',0xCA:'SOF10 prog-arith',
     0xCB:'SOF11',0xCD:'SOF13',0xCE:'SOF14',0xCF:'SOF15'}
def jpeg(b):
    i=2; f={'sof':None,'comps':None,'samp':None,'prec':None,'dri':0,'nDHT':0,'nDQT':0,
           'app':set(),'nSOS':0,'size':None}
    while i<len(b)-1:
        if b[i]!=0xFF: i+=1; continue
        m=b[i+1]; i+=2
        if m in (0xD8,0x01) or 0xD0<=m<=0xD7: continue
        if m==0xD9: break
        if i+2>len(b): break
        L=struct.unpack('>H',b[i:i+2])[0]; seg=b[i+2:i+L]
        if m in SOF:
            f['sof']=SOF[m]; f['prec']=seg[0]
            h,w=struct.unpack('>HH',seg[1:5]); f['size']=(w,h)
            n=seg[5]; f['comps']=n
            f['samp']='x'.join(f"{seg[6+3*k+1]>>4}{seg[6+3*k+1]&15}" for k in range(n))
        elif m==0xC4: f['nDHT']+=1
        elif m==0xDB: f['nDQT']+=1
        elif m==0xDD: f['dri']=struct.unpack('>H',seg[:2])[0]
        elif 0xE0<=m<=0xEF: f['app'].add(f"APP{m-0xE0}:"+seg[:4].decode('latin1').strip('\x00'))
        elif m==0xDA:
            f['nSOS']+=1
            i+=L
            while i<len(b)-1:
                if b[i]==0xFF and b[i+1]!=0 and not (0xD0<=b[i+1]<=0xD7): break
                i+=1
            continue
        i+=L
    return f
def png(b):
    i=8; f={'chunks':[],'ihdr':None}
    while i+8<=len(b):
        L=struct.unpack('>I',b[i:i+4])[0]; t=b[i+4:i+8].decode('latin1')
        f['chunks'].append(t)
        if t=='IHDR':
            w,h,d,c,cm,fl,il=struct.unpack('>IIBBBBB',b[i+8:i+8+13])
            f['ihdr']=dict(w=w,h=h,depth=d,color=c,interlace=il,filter=fl,compress=cm)
        if t=='IEND': break
        i+=12+L
    return f
rows=[]
for p in sorted(sys.argv[1:]):
    b=open(p,'rb').read()
    if b[:2]==b'\xff\xd8': rows.append(('JPEG',p,jpeg(b)))
    elif b[:8]==b'\x89PNG\r\n\x1a\n': rows.append(('PNG',p,png(b)))
    else: rows.append(('OTHER',p,{'head':b[:16]}))
j=[r for r in rows if r[0]=='JPEG']; p_=[r for r in rows if r[0]=='PNG']
print(f"== {len(j)} JPEG ==")
agg=collections.Counter()
for _,p,f in j:
    agg[(f['sof'],f['prec'],f['comps'],f['samp'],f['dri']>0,f['nSOS'],tuple(sorted(f['app'])),f['size'])]+=1
for k,v in agg.most_common():
    print(f"  n={v:<3} SOF={k[0]} prec={k[1]} comps={k[2]} sampling={k[3]} restart={k[4]} nSOS={k[5]} size={k[7]} app={k[6]}")
print(f"  DHT tables/file: {sorted(set(f['nDHT'] for _,_,f in j))}  DQT: {sorted(set(f['nDQT'] for _,_,f in j))}")
print(f"== {len(p_)} PNG ==")
agg=collections.Counter()
for _,p,f in p_:
    h=f['ihdr']; agg[(h['depth'],h['color'],h['interlace'],tuple(dict.fromkeys(f['chunks'])),(h['w'],h['h'])) ]+=1
for k,v in agg.most_common():
    ct={0:'gray',2:'RGB',3:'palette',4:'gray+A',6:'RGBA'}[k[1]]
    print(f"  n={v:<3} depth={k[0]} color={k[1]}({ct}) interlace={k[2]} size={k[4]} chunks={list(k[3])}")
o=[r for r in rows if r[0]=='OTHER']
if o: print(f"== {len(o)} non-image ==")

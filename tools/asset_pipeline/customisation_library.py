"""Prepare resident CAC geometry and shared textures outside the game loop."""
from pathlib import Path
import copy,json,os,shutil,sys,re,struct
from concurrent.futures import ThreadPoolExecutor
import numpy as np
from PIL import Image,ImageChops
if __name__=='__main__':sys.path.insert(0,str(Path(__file__).resolve().parents[2]))  # a fanout worker (prepare's lanes)
from tools.asset_pipeline.customisation_worker import ROOT,default_profile,flag
from tools.asset_pipeline.customisation_catalog import write_private
from tools.owned_game.big import BigArchive
from tools.asset_pipeline.optional_content import CONTENT_ERRORS

def friendly(name):
    text=name.lower().replace('_',' ')
    replacements=[('shortsleeve','short sleeve'),('longsleeve','long sleeve'),('tshirt','tee'),
        ('t-shirt','tee'),('buttonup','button-up'),('zipup','zip-up'),('flippedup','flipped up'),
        ('kneedown',''),('thighdown',''),('medhem',''),('lowhem',''),('highhem',''),
        ('sleeves pushed up','rolled sleeves'),('sleeves up','rolled sleeves'),('sleevup','rolled sleeves'),
        ('colourizable',''),('colorizable',''),('stampable',''),('logoable',''),('material',''),
        ('lambert1','original'),('newera','new era'),('45right','angled right'),('45left','angled left'),
        ('long sleeve tee','long-sleeve tee'),('short sleeve tee','tee'),
        ('short sleeve button-up shirt','short-sleeve shirt'),('long sleeve button-up shirt','long-sleeve shirt'),
        ('shirt ls button-up','long-sleeve shirt'),('top button only','top button'),
        ('long sleeve mens shirt vneck','long-sleeve v-neck'),('jeans straight leg baggy waistline','baggy straight jeans'),
        ('jeans straight leg baggy waistline','baggy straight jeans'),('sunglasses','shades'),('necklaces',''),('tattoo',''),('biglizard','big lizard'),
        ('blueflame','blue flame'),('radialsun','sunburst'),('tribalbody','tribal'),('pinstripe','pinstripe ')]
    for a,b in replacements:text=text.replace(a,b)
    text=re.sub(r'\b(male\d*|female\d*|unisex|mesh|mat|hmat|h|l|high|low|ropa|hem|inner)\b','',text)
    text=re.sub(r'\b(?:0x)?[a-f0-9]{8,}\b','',text)
    text=re.sub(r'([a-z])([0-9]+)\b',r'\1 \2',text)
    text=re.sub(r'\s+',' ',text).strip(' -')
    text=text.replace('jeans straight leg baggy waistline','baggy straight jeans').replace('long-sleeve tee rolled sleeves','rolled-sleeve tee')
    return text.title() or 'Original'


def graphic_kind(native_name):
    """Edit Skater's Graphic tabs: logo_* are Brand Logos, team_* Team Logos; logoEmpty_* are blanks."""
    name=native_name.lower()
    return 'team' if name.startswith('team_') else 'brand' if name.startswith('logo_') else None

GRAPHIC_WORDS={'bw':'B&W','dc':'DC','dvs':'DVS','dgk':'DGK','sv':'SV','es':'ES','lrg':'LRG','rvca':'RVCA','c1rca':'C1RCA',
    'wesc':'WeSC','hifi':'Hi-Fi','planb':'Plan B','santacruz':'Santa Cruz','alienworkshop':'Alien Workshop','newera':'New Era',
    'nikesb':'Nike SB','zooyork':'Zoo York','trasher':'Thrasher','theskateboardmag':'The Skateboard Mag','shakejunt':'Shake Junt',
    'blackbox':'Black Box','primaryskate':'Primary Skate','strikeforcedelta':'Strike Force Delta','inlipswetrust':'In Lips We Trust',
    'hurdygurdy':'Hurdy Gurdy','coolshades':'Cool Shades','goatskull':'Goat Skull','dentureman':'Denture Man',
    'machinegun':'Machine Gun','bandaid':'Band-Aid','skatemental':'Skate Mental'}

def graphic_name(native_name):
    """logo_fourstar2_bw_mat -> 'Fourstar B&W 2'."""
    words=re.sub(r'^(team_brand_logo_|team_logo_|logo_)|_mat$','',native_name.lower()).split('_');number=''
    for i,word in enumerate(words):
        variant=re.fullmatch(r'([a-z]+?)(\d)',word)
        if variant:words[i]=variant[1];number=number or ('' if variant[2]=='1' else ' '+variant[2])
    return ' '.join(GRAPHIC_WORDS.get(w,w.capitalize()) for w in words if w)+number

def variant_label(slot, model_name, native_name):
    label=friendly(native_name)
    worn=bool(re.search(r'(?:_| )d(?:_| |$)|dirty',native_name.lower()))
    label=re.sub(r'\b(Diffuse|Tga|D|Malemat|Skateboard|Featured Artist|Instance)\b','',label)
    label=re.sub(r'\b0\b','',label)
    if slot=='Feet': return 'Worn' if worn else 'New'
    model_words=set(friendly(model_name).lower().split())
    words=[w for w in label.split() if w.lower() not in model_words]
    label=' '.join(words).strip()
    for a,b in [('Ozzie Wright','Ozzie'),('Grass Roots Pullover Hoodie','Grass Roots'),
                ('Full Back New Era',''),('Hoodie Hooddown Pullover',''),('Hood Up',''),
                ('Hooddown',''),('Closed',''),('Open',''),('In4Mation Colab Zip','In4Mation'),
                ('Nightmare Catcher','Nightmare'),('Gonz Animal Bullfish','Bullfish')]: label=label.replace(a,b)
    label=re.sub(r'\s+',' ',label).strip()
    if not label: label='Original'
    return label+(' (Worn)' if worn else '')


def prune_unavailable(models, materials, errors):
    """Never expose a model selection that points at an absent material."""
    for key, model in list(models.items()):
        model['groups'] = [[mid for mid in group if mid in materials] for group in model['groups']]
        if not model['groups'] or any(not group for group in model['groups']):
            del models[key]
            errors.append(dict(model=key, error='No available material for a mesh group'))
        else:
            model['materials'] = model['groups'][0]


def _save(image,dest,**params):
    """image.save(dest) through a temporary name, so a stopped worker never leaves a half PNG that exists() trusts."""
    temporary=dest.with_name(dest.name+'.part')
    try:image.save(temporary,'PNG',**params)
    except BaseException:
        temporary.unlink(missing_ok=True);raise
    os.replace(temporary,dest)

def decode_png(parser,source,destination,name=None):
    """extract_default_skater.decode_texture without its re-read of the PNG (a checksum no caller here uses); source
    may be the .rx2's bytes (`name` its file name, for the error). Returns the decoded image."""
    parsed=parser.parse_rx2(source)
    if len(parsed.textures)!=1:raise RuntimeError(f"Expected one texture in {name or source.name}, found {len(parsed.textures)}")
    destination.parent.mkdir(parents=True,exist_ok=True)
    image=parsed.textures[0].to_pil();_save(image,destination,compress_level=1)  # RX2Texture.save_png
    return image

def tangent_normals(a):
    """An RGBA normal map's unit tangent normals (x, y, z). Most are two-channel (x in alpha, y in green, z rebuilt);
    some are plain RGB with an empty (opaque) alpha, whose x is red: read as alpha, x was 1 and every normal lay flat
    along the surface (the Deer Man of Dark Woods gloves and 113 more maps)."""
    x=(a[:,:,0] if a[:,:,3].min()>=254 else a[:,:,3])/127.5-1.;y=a[:,:,1]/127.5-1.;scale=np.maximum(np.sqrt(x*x+y*y),1.)
    x=x/scale;y=y/scale
    return np.stack((x,y,np.sqrt(np.maximum(0.,1.-x*x-y*y))),axis=2)

def _derive(src,dest,kind,decoded=None):
    """src's normal or rough map; decoded: src's image when it was just decoded (an RGBA image is what its PNG reads back)."""
    image=(decoded if decoded is not None and decoded.mode=='RGBA' else Image.open(src)).convert('RGBA');a=np.array(image,dtype=np.float64)
    if kind=='normal':
        image=Image.fromarray(np.rint((tangent_normals(a)*.5+.5)*255.).astype('uint8'))
    elif kind=='rough':
        grey=ImageChops.invert(image.convert('L'));white=Image.new('L',image.size,255);image=Image.merge('RGB',(white,grey,white))
    _save(image,dest)

# The shaders whose pixel shader samples i_alpha (shaders_final.big): cac_alpha_defaultPS and cac_hair_defaultPS both
# take coverage from its red channel on TEXCOORD1 (their VS is the same: UV0 in xy, UV1 in zw), times i_params.x, and use
# the diffuse alpha only as the specular mask (squared). cacstamp_skin_defaultPS has no i_alpha: a skin material's alpha
# map is never read.
COVERAGE={'cac_alpha','cac_hair','cac_hair_ropa'}

def _recipe_base():
    recipe_base=json.loads((ROOT/'tools/default_skater_retail_manifest.json').read_text())
    recipe_base['morph_assembly']['live_targets']=['fat','thin']+recipe_base['morph_assembly']['face_targets']+['global_ethnicity_african','global_ethnicity_asian','global_ethnicity_caucasian']
    return recipe_base

FACE_JOINTS=['FACE','OFFSET_JAW','JAW','OFFSET_CHIN','CHIN','TONGUE','TONGUETIP',
    'LEFTEYE','RIGHTEYE','LEFTUPEYELID','RIGHTUPEYELID','LEFTLOWEYELID','RIGHTLOWEYELID',
    'LEFTINNEREYEBROW','RIGHTINNEREYEBROW','LEFTOUTEREYEBROW','RIGHTOUTEREYEBROW',
    'LEFTCHEEK','RIGHTCHEEK','LEFTUPCHEEK','RIGHTUPCHEEK','LEFTNOSE','RIGHTNOSE',
    'LEFTCREASE','RIGHTCREASE','LEFTMOUTH','RIGHTMOUTH',
    'UPPERLIP','LOWERLIP','LEFTUPPERLIP','RIGHTUPPERLIP','LEFTLOWERLIP','RIGHTLOWERLIP']
# Skate 2's cutscene clips key these on the player (faces.md 4B); Skate 3's CAS head (Rostral) and eyes (Organ) carry them.
FACE_SLOTS={'Rostral','Organ'}

def _rig(assets,out):
    from tools.asset_pipeline.retail_character import AnimSource,SkeletonSet
    # Read-only shared rig inputs live only for this preparation invocation.
    # Per-item meshes and morphs are discarded after conversion, bounding RAM.
    reference=out/'reference'
    return _recipe_base(),reference,AnimSource(assets/'private/stock/data/anim/OnBoard.abin'),SkeletonSet(str(reference))

def _convert(assets,out,slot,key,source,rig):
    from tools.asset_pipeline.retail_character import RX2,decode_dense_morphs
    from tools.asset_pipeline.character_glb import convert
    recipe_base,reference,animation,reference_skeleton=rig
    path=out/(key+'_v4.glb');folder=out/'work'/key/'models'/slot;folder.mkdir(parents=True,exist_ok=True)
    mesh_file=folder/source.name
    if not mesh_file.exists():shutil.copyfile(source,mesh_file)
    parsed=RX2.parse_rx2(str(mesh_file));mesh=next(m for m in parsed['meshes'] if m.get('positions') and m.get('indices'))
    morphs=decode_dense_morphs(mesh_file,parsed,len(mesh['positions']),RX2)
    recipe=copy.deepcopy(recipe_base);recipe['components']=[dict(slot=slot,textures={},tint=[1.,1.,1.],grip=slot=='SkateBoard')]
    recipe['morph_assembly']['expected_targets']={slot:[m['name'] for m in morphs]}
    temporary=path.with_suffix('.tmp')
    convert(folder.parent,assets/'private',recipe,output=temporary,geometry_only=True,live_morphs=True,reference_models=reference,
            source=animation,reference_skeleton=reference_skeleton,
            parsed_models={str(mesh_file):parsed},decoded_morphs={str(mesh_file):morphs},
            extra_bones=FACE_JOINTS if slot in FACE_SLOTS else None)
    temporary.replace(path)

def _gripped(path):
    """Whether a deck's GLB has its grip cut (character_glb.py); one converted before it is converted again."""
    with open(path,'rb') as f:
        head=f.read(20);text=f.read(struct.unpack_from('<I',head,12)[0])
    return json.loads(text).get('asset',{}).get('extras',{}).get('grip')==1

def _work(jobs,assets,cache):
    """Planned jobs (prepare's walk lists them): models to GLB, textures decoded with their normal/rough maps.
    A job that fails is left undone: prepare's own pass then meets the same error and
    reports it as it always did."""
    from tools.extract_default_skater import import_rx2_parser
    out=cache/'library';parser=import_rx2_parser(ROOT/'tools/vendor/utt');rig=None
    def image_job(job):
        try:
            src=cache/'decoded'/(job[1]+'.png')
            decoded=decode_png(parser,Path(job[2]),src) if job[2] else None
            for kind in job[3]:_derive(src,out/'textures'/(job[1]+'_'+kind+'.png'),kind,decoded)
        except Exception:
            pass
    for job in jobs:
        if job[0]=='glb':
            try:
                rig=rig or _rig(assets,out)
                _convert(assets,out,job[2],job[1],Path(job[3]),rig)
            except Exception:
                pass
    # ponytail: 4 threads, each job its own files: the decode, PNG zlib and numpy let go of the GIL (a 2048px map's float
    # copy is ~130 MB, so not more)
    with ThreadPoolExecutor(4) as pool:list(pool.map(image_job,[job for job in jobs if job[0]!='glb']))

def _run(jobs,assets,cache,skate1):
    """The jobs on fanout's worker processes (this file run with --lane), or here when there are few. A lane costs ~1.3 s
    to start and ~0.45 GB: under ~400 units of work (~3 s) one process is as fast (10-08, the DLC set: 0.6 GB, not 3.2)."""
    from tools.pipeline import fanout
    weight={'glb':6,'tex':1}
    size=lambda job:weight[job[0]]+(len(job[3]) if job[0]=='tex' else 0)
    if len(jobs)<16 or sum(map(size,jobs))<400 or fanout.workers()<2:
        return _work(jobs,assets,cache)
    with fanout.fan_out(__file__,jobs,(assets,cache,int(skate1)),size=size):
        pass

def prepare(config):
    from tools.extract_default_skater import import_rx2_parser,parse_fallback_recipe
    from tools.asset_pipeline.retail_character import RX2
    assets=Path(config['assets']);cache=Path(config.get('directory',assets/'private/customisation'));out=cache/'library';out.mkdir(exist_ok=True)
    catalog=json.loads((cache/'catalog.json').read_text());archive=BigArchive(Path(config['game_root'])/'data/content/createacharacter.big')
    entries={e.path.lower():e for e in archive.entries}
    plan=None  # a dict while walk() only lists the work it finds (the parallel passes below); None: walk() does the work
    made={}  # file a planned job made -> the source extracted for it (or None); removed at the end unless the real pass used it
    used=set()
    pending={}  # planned extractions: source file -> archive entry (extracted by flush, before the workers need them)
    def extract(path):
        dest=cache/'source'/path
        if not dest.exists():
            if plan is None:write_private(dest,archive.read(entries[path.lower()]))
            elif dest not in made:pending[dest]=entries[path.lower()];made[dest]=None
        if plan is None:used.add(dest)  # the sequential pass extracts it here: kept
        return dest
    def flush():
        """The planned extractions, 8 threads (file reads, refpack and writes); one that fails is left to the real pass,
        which meets the same error and reports it."""
        def one(item):
            try:write_private(item[0],archive.read(item[1]))
            except Exception:pass
        with ThreadPoolExecutor(8) as pool:list(pool.map(one,pending.items()))
        pending.clear()
    def mark(path):
        used.add(path)
        if made.get(path):used.add(made[path])
    parser=import_rx2_parser(ROOT/'tools/vendor/utt');decoded=cache/'decoded';decoded.mkdir(exist_ok=True)
    def texture(tid,kind='raw'):
        src=decoded/(tid+'.png')
        if not src.exists():
            source=extract('data/content/createacharacter/texture/0x'+tid+'.rx2')
            if plan is None:decode_png(parser,source,src)
            else:plan['tex'].setdefault(tid,['tex',tid,str(source),[]]);made[src]=source
        mark(src)
        if kind=='raw':return src.relative_to(assets).as_posix()
        dest=out/'textures'/(tid+'_'+kind+'.png');dest.parent.mkdir(exist_ok=True)
        if not dest.exists():
            if plan is None:_derive(src,dest,kind)
            else:
                job=plan['tex'].setdefault(tid,['tex',tid,None,[]]);made[dest]=None
                if kind not in job[3]:job[3].append(kind)
        mark(dest)
        return dest.relative_to(assets).as_posix()
    recipe_base=_recipe_base()
    reference=out/'reference'
    for component in recipe_base['components']:
        slot=component['slot'];model=next(m for p in catalog['components'] if p['slot']==slot for m in p['models'] if m['id']==component['asset_id'])
        src=extract(next(l for l in model['lods'] if l['index']==0)['path']);dest=reference/slot/src.name
        dest.parent.mkdir(parents=True,exist_ok=True)
        if not dest.exists():shutil.copyfile(src,dest)
    defaults={x['material_id']:x for x in recipe_base['components']}
    def walk(rig=None):
        model_data={};mat_data={};errors=[];tattoos={};graphics={}
        models_only=plan is not None and 'glb' in plan
        for part in catalog['components']:
            for model in part['models']:
                if part['slot']=='Misc':continue  # Non-geometric stamp catalogue, indexed below.
                slot=part['slot'];key=model['id'];path=out/(key+'_v4.glb')
                try:
                    lod=next(l for l in model['lods'] if l['index']==0)
                    if not path.exists() or (slot=='SkateBoard' and not _gripped(path)):
                        if plan is None:_convert(assets,out,slot,key,extract(lod['path']),rig)
                        elif models_only:source=extract(lod['path']);plan['glb'].setdefault(key,['glb',key,slot,str(source)]);made[path]=source
                        elif not path.exists():continue  # its conversion failed: so will the real pass's, which then skips its designs
                    if models_only:continue  # models first, their designs once the models exist
                    mark(path)  # a planned model keeps its extracted mesh, as the sequential pass does
                    groups=[[v['id'] for v in group] for group in lod['material_instances']]
                    model_data[key]=dict(slot=slot,name=friendly(model['name']),flags={k[4:]:v for k,v in model['flags'].items() if v},
                        materials=groups[0],groups=groups,scene=path.relative_to(assets).as_posix())
                    for v in (v for group in lod['material_instances'] for v in group):
                        try:
                            mid=v['id']
                            if mid in mat_data:continue
                            mat=catalog['materials'][mid];tex={t['channel']:t['id'] for t in mat['textures']}
                            if 'diffuse' not in tex:continue
                            diffuse=texture(tex['diffuse']);covered='alpha' in tex and mat.get('shader') in COVERAGE
                            mat_data[mid]=dict(name=variant_label(slot,model['name'],v['name']),flags={k[4:]:val for k,val in mat['flags'].items() if val},
                                diffuse=diffuse,normal=texture(tex['normal'],'normal') if 'normal' in tex else None,
                                rough=texture(tex['specular'],'rough') if 'specular' in tex else None,
                                opacity=texture(tex['alpha']) if covered else None,alpha=covered,tint=defaults.get(mid,{}).get('tint',
                                    [0.72,0.57,0.49] if mat['flags'].get('cas.SkinTone')=='light' else
                                    [0.33,0.26,0.23] if mat['flags'].get('cas.SkinTone')=='dark' else
                                    [0.02,0.01,0.01] if slot=='Hair' else [1.,1.,1.]),
                                metallic=.65 if slot=='SkateTruck' else 0.,roughness=.48 if slot in {'SkateTruck','SkateWheel'} else .72)
                        except CONTENT_ERRORS as error:
                            errors.append(dict(model=key,material=v.get('id'),slot=slot,error=str(error)))
                except CONTENT_ERRORS as e:errors.append(dict(model=key,slot=slot,error=str(e)))
            if plan is None:print('Prepared',part['slot'],len(model_data),'models',len(mat_data),'materials',flush=True)
        if models_only:return
        misc=next((p for p in catalog['components'] if p['slot']=='Misc'),None)
        variants=(v for m in (misc or {}).get('models',[]) for lod in m.get('lods',[])
                  for group in lod.get('material_instances',[]) for v in group)
        for variant in variants:
            try:
                mid=variant['id'];mat=catalog['materials'][mid]
                tex={t['channel']:t['id'] for t in mat['textures']}
                kind=graphic_kind(variant['name'])
                if kind and not mat['flags'].get('cas.TattooCategory'):
                    graphics[mid]=dict(name=graphic_name(variant['name']),kind=kind,texture=texture(tex['decal']),
                        bounds=[float(v) for v in mat['flags']['cas.StampBorderConstraint'].split(',')])
                if not mat['flags'].get('cas.TattooCategory'):continue
                tattoos[mid]=dict(name=friendly(variant['name']),texture=texture(tex['decal']),
                    bounds=[float(v) for v in mat['flags']['cas.StampBorderConstraint'].split(',')])
            except CONTENT_ERRORS as error:
                errors.append(dict(tattoo=variant.get('id'),error=str(error)))
        return model_data,mat_data,errors,tattoos,graphics
    # ponytail: the work is listed by walking the catalog as the real pass does (models first, then their designs), done
    # by worker processes, and the real pass then finds it done. A planned file the real pass never reaches (a design after
    # a texture that fails to decode) is removed again: the files are what the sequential pass alone leaves.
    skate1=getattr(RX2.parse_rx2,'skate1',False)  # Skate 1 meshes (s1mesh.install): the workers read them alike
    plan={'glb':{}};walk();flush()
    _run(list(plan['glb'].values()),assets,cache,skate1)
    plan={'tex':{}};walk();flush()
    _run(list(plan['tex'].values()),assets,cache,skate1)
    plan=None;used.clear()
    model_data,mat_data,errors,tattoos,graphics=walk(_rig(assets,out))
    for path in made:
        if path not in used:path.unlink(missing_ok=True)
    prune_unavailable(model_data,mat_data,errors)
    male=default_profile();male['gender']='male'
    female=copy.deepcopy(male);female['gender']='female';female['selections']={}
    raw=parse_fallback_recipe(assets/'private/stock/data/cacrecipes/SavedRecipeFallbackFemale.bin',16)
    for slot,items in raw['asset_lists'].items():
        if items:
            item=items[0];female['selections'][slot]={'asset_id':item['asset_id'],'material_id':item['models'][0]['material_id']}
    colours=[]
    native=json.loads((cache/'native.json').read_text())
    for row in native['collections']:
        if row['class']=='cac_colour' and row['key'].endswith('_clothing') and row['key']!='default_clothing':
            colours.append(dict(name=row['key'][:-9].replace('_',' ').title(),
                rgb=struct.unpack('>4f',bytes.fromhex(row['fields']['Hash_7827ED970A88B70C']['data']))[:3]))
    colours.sort(key=lambda c:c['name'])
    library=dict(version=3,colours=colours,tattoos=tattoos,graphics=graphics,models=model_data,materials=mat_data,defaults={'male':male,'female':female},
                 morphs=recipe_base['morph_assembly']['live_targets'],errors=errors)
    (cache/config.get('library_index','library.json')).write_text(json.dumps(library,separators=(',',':')))
    print('LIBRARY_READY',len(model_data),len(mat_data),'errors',errors,flush=True)
    return library

if __name__=='__main__':
    if sys.argv[1]=='--lane':  # a fanout worker: --lane PLAN --out DIR ASSETS CACHE SKATE1
        from tools.pipeline import fanout
        lane,_,(assets,cache,skate1)=fanout.lane(sys.argv[1:])
        if skate1=='1':
            sys.path.insert(0,str(ROOT/'tools/pipeline/skate2port'))
            import s1mesh
            from tools.asset_pipeline import retail_character
            s1mesh.install(retail_character.RX2,retail_character)
        _work([job for _,job in lane],Path(assets),Path(cache))
    else:prepare(json.loads(Path(sys.argv[1]).read_text()))

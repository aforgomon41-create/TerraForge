"""Build the bilingual catalogue from the pinned IUCN team's MIT data extract.
No network access. --check validates committed generated output.
"""
import argparse
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DATA = ROOT/'data/ecosystems'
BIOMES = '''河流与溪流
湖泊
人工湿地
半封闭过渡水域
海洋陆架
远洋水体
深海海底
人工海洋生态
咸淡水潮汐带
海岸线
潮上带沿海生态
人工海岸
地下岩质生态
人工地下空间
地下淡水
人工地下淡水
地下潮汐生态
热带—亚热带森林
温带—寒带森林与疏林
灌丛与灌木疏林
稀树草原与草地
荒漠与半荒漠
极地与高山寒冻生态
集约土地利用
沼泽湿地'''.splitlines()
GROUPS = '''F1.1|常流水高地溪流
F1.2|常流水低地河流
F1.3|冻融河流与溪流
F1.4|季节性高地溪流
F1.5|季节性低地河流
F1.6|干旱区偶发河流
F1.7|大型低地河流
F2.1|大型常年淡水湖
F2.2|小型常年淡水湖
F2.3|季节性淡水湖
F2.4|冻融淡水湖
F2.5|短暂性淡水湖
F2.6|常年盐湖与碱湖
F2.7|短暂性盐湖
F2.8|承压泉与绿洲
F2.9|地热水池与湿地
F2.10|冰下湖
F3.1|大型水库
F3.2|人工湖泊湿地
F3.3|稻田
F3.4|淡水养殖场
F3.5|运河、沟渠与排水道
FM1.1|深水沿海海湾
FM1.2|常开口河口与海湾
FM1.3|间歇开闭的沿海湖泊与潟湖
M1.1|海草床
M1.2|海藻林
M1.3|透光带珊瑚礁
M1.4|贝类床与贝礁
M1.5|弱光海洋动物林
M1.6|潮下带岩礁
M1.7|潮下带沙底
M1.8|潮下带泥质平原
M1.9|上升流区
M1.10|钙化红藻床
M2.1|表层远洋水体
M2.2|中层远洋水体
M2.3|深层远洋水体
M2.4|深渊远洋水体
M2.5|海冰生态
M3.1|大陆与岛屿斜坡
M3.2|海底峡谷
M3.3|深海平原
M3.4|海山、洋脊与海台
M3.5|深水生物成因底栖床
M3.6|超深海沟与海槽
M3.7|化能合成生态系统
M4.1|水下人工结构
M4.2|海水养殖场
MFT1.1|沿海河流三角洲
MFT1.2|潮间带森林与灌丛
MFT1.3|沿海盐沼与芦苇湿地
MT1.1|岩质海岸
MT1.2|泥质海岸
MT1.3|沙质海岸
MT1.4|巨砾与卵石海岸
MT2.1|沿海灌丛与草地
MT2.2|大型海鸟与鳍足类聚居地
MT3.1|人工海岸线
S1.1|有氧洞穴
S1.2|岩内生态
S2.1|人工地下洞室
SF1.1|地下溪流与水池
SF1.2|地下水生态
SF2.1|水管与地下运河
SF2.2|淹水矿井与其他洞室
SM1.1|陆海连通咸水洞穴
SM1.2|陆海连通咸水池
SM1.3|海蚀洞穴
T1.1|热带—亚热带低地雨林
T1.2|热带—亚热带干旱林与密灌丛
T1.3|热带—亚热带山地雨林
T1.4|热带石南林
T2.1|寒带及温带高山森林与疏林
T2.2|温带落叶林
T2.3|海洋性冷温带雨林
T2.4|暖温带照叶林
T2.5|温带火适应湿润林
T2.6|温带火适应硬叶林与疏林
T3.1|季节干旱热带灌丛
T3.2|季节干旱温带石南与灌丛
T3.3|冷温带石南荒原
T3.4|新生岩面、熔岩流与碎石坡
T4.1|养分与食草作用主导的稀树草原
T4.2|火作用主导的丛生草稀树草原
T4.3|丘状草稀树草原
T4.4|温带疏林
T4.5|温带半湿润草地
T5.1|半荒漠草原
T5.2|多肉或多刺植被荒漠与半荒漠
T5.3|硬叶热荒漠与半荒漠
T5.4|冷荒漠与半荒漠
T5.5|极端干旱荒漠
T6.1|冰盖、冰川与常年雪原
T6.2|极地—高山崖壁、碎石坡、露岩与熔岩流
T6.3|极地苔原与荒漠
T6.4|温带高山草地与灌丛
T6.5|热带高山草地与草本群落
T7.1|一年生作物农田
T7.2|人工播种牧草地
T7.3|人工种植园
T7.4|城市与工业生态
T7.5|次生半自然牧场与弃耕地
TF1.1|热带淹水林与泥炭林
TF1.2|亚热带—温带森林湿地
TF1.3|常年沼泽
TF1.4|季节性洪泛沼泽
TF1.5|干旱区偶发洪泛地
TF1.6|寒带、温带与山地高位泥炭沼泽
TF1.7|寒带与温带低位泥炭沼泽'''
ESTIMATED = set('F1.3 F2.1 F2.2 F2.4 M2.1 M2.5 T1.1 T1.2 T1.3 T2.1 T2.2 T2.3 T2.4 T3.1 T3.2 T4.5 T5.1 T5.4 T5.5 T6.1 T6.2 T6.3 T6.4 T6.5 TF1.3'.split())
COLORS = ['548ca1','4c91aa','8e9eaa','629f9e','578e9a','376e87','3a526e','8c899e','729678','b5ae8d','a3ae79','ad9e98','777783','8a7f82','64828b','8a969f','6d7c90','327347','4e784e','89935a','b2b368','c9b383','d9e4dc','b19b70','5d9985']

def build():
    source=json.loads((DATA/'source.json').read_text(encoding='utf8'))
    translations=dict(line.split('|',1) for line in GROUPS.splitlines())
    biomes=[dict(x,id=i+1,name_zh=BIOMES[i],color='#'+COLORS[i]) for i,x in enumerate(source['biomes'])]
    ids={b['code']:b['id'] for b in biomes}
    groups=[]
    for g in source['groups']:
        code=g['code']; parent=ids[g['biome']]
        support=('human_activity' if g['biome'] in ['F3','M4','MT3','S2','SF2','T7'] else
                 'subsurface' if code.startswith(('S1.','SF1.','SM1.')) else
                 'environment_estimate' if code in ESTIMATED else 'additional_process')
        groups.append(dict(g,id=parent*16+int(code.split('.')[1]),biome_id=parent,
                           name_zh=translations[code],support=support))
    assert len(biomes)==25 and len(groups)==110 and len(translations)==110
    assert len({g['id'] for g in groups})==110
    return dict(version='iucn-get-v2.1-tf1',source=source['source'],biomes=biomes,groups=groups)

if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--check',action='store_true');args=parser.parse_args()
    output=json.dumps(build(),ensure_ascii=False,indent=2)+'\n';path=DATA/'catalogue.json'
    if args.check:assert path.read_text(encoding='utf8')==output,'Catalogue needs rebuilding'
    else:path.write_text(output,encoding='utf8')
    print('Verified 25 biomes and 110 unique functional groups')

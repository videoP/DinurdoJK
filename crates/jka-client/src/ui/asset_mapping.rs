//! Native Asset Mapping page. Taffy supplies the ONLY layout/hit rectangles;
//! WGPU draws them, and winit consumes the same geometry in app::asset_mapping.
//! No egui widget or dependency on mounted JK2/JKA packages is needed.
use super::{flex::{self, Bounds, Container, Flex}, rect, rect_outline, text::UiText, UiSnapshot, UiVertex};
use taffy::prelude::{AlignItems, JustifyContent, NodeId};

const WHITE: [f32; 4] = [0.91, 0.96, 1.0, 1.0];
const DIM: [f32; 4] = [0.65, 0.76, 0.86, 1.0];
const CYAN: [f32; 4] = [0.36, 0.82, 1.0, 1.0];
const GREEN: [f32; 4] = [0.35, 0.96, 0.59, 1.0];
const RED: [f32; 4] = [1.0, 0.42, 0.40, 1.0];
const AMBER: [f32; 4] = [1.0, 0.76, 0.42, 1.0];

#[derive(Clone, Debug, Default)]
pub struct FolderUi {
    pub address: String,
    pub target: usize,
    pub entries: Vec<PickerEntryUi>,
    pub scroll: usize,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Default)]
pub struct PickerEntryUi { pub name: String, pub is_dir: bool }
#[derive(Clone, Debug, Default)]
pub struct AssetMappingUi {
    /// 0 JKA base; 1 JKA CD; 2 JK2 base; 3 JK2 CD;
    /// 4 JKA forcegame; 5 JK2 forcegame.
    pub values: [String; 6],
    pub valid: [Option<bool>; 4],
    pub force_status: [Option<bool>; 2],
    pub force_invalid: [bool; 2],
    pub retail: [bool; 4],
    pub jk2_status: [String; 2],
    pub feedback: Option<String>,
    pub allow_close: bool,
    pub focus: Option<usize>,
    pub caret: usize,
    pub selection_anchor: Option<usize>,
    pub text_start: usize,
    pub scroll: f32,
    pub pointer: [f32; 2],
    pub picker: Option<FolderUi>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FieldLayout { pub label: Bounds, pub edit: Bounds, pub browse: Bounds, pub status: Bounds }
#[derive(Clone, Debug)]
pub struct Layout {
    pub header: Bounds, pub title: Bounds, pub subtitle: Bounds,
    pub viewport: Bounds, pub jka: Bounds, pub jk2: Bounds, pub override_panel: Bounds,
    pub jka_title: Bounds, pub jk2_title: Bounds, pub force_title: Bounds,
    pub retail: Bounds, pub jk2_status: Bounds, pub feedback: Bounds,
    pub fields: [FieldLayout; 6],
    pub footer: Bounds, pub save: Bounds, pub restart: Bounds, pub back: Bounds,
    pub scroll_max: f32, pub scroll: f32, pub scale: f32,
    pub picker: Option<PickerLayout>,
}
#[derive(Clone, Copy, Debug)]
pub struct PickerLayout {
    pub panel: Bounds, pub title: Bounds, pub address: Bounds,
    pub parent: Bounds, pub refresh: Bounds, pub choose: Bounds, pub cancel: Bounds,
    pub list: Bounds, pub error: Bounds, pub row_h: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Edit(usize), Browse(usize), Save, Restart, Back,
    Address, Parent, Refresh, Choose, Cancel, Entry(usize),
}

fn translated(mut b: Bounds, y: f32) -> Bounds { b[1] -= y; b }
fn visible(b: Bounds, clip: Bounds) -> bool {
    b[1] >= clip[1] && b[1] + b[3] <= clip[1] + clip[3]
}
fn panel_row(f: &mut Flex, width: f32, s: f32, browse: bool) -> (NodeId, [NodeId; 4]) {
    let label = f.band(26.0*s);
    let edit = f.leaf((width - if browse { 114.0*s } else { 0.0 }).max(20.0), 44.0*s);
    let button = f.leaf(if browse { 106.0*s } else { 0.0 }, 44.0*s);
    let controls = if browse {
        f.node(Container::row().width(width).height(44.0*s).gap(8.0*s), &[edit, button])
    } else {
        f.node(Container::row().width(width).height(44.0*s), &[edit])
    };
    let status = f.band(26.0*s);
    let row = f.node(Container::column().width(width).height(104.0*s).gap(4.0*s), &[label,controls,status]);
    (row, [label,edit,button,status])
}
fn section(f: &mut Flex, width: f32, s: f32, title_h: f32, index_a: usize, index_b: usize,
    field_nodes: &mut [[Option<NodeId>;4];6]) -> (NodeId,NodeId,NodeId) {
    let inner = (width-36.0*s).max(1.0);
    let title = f.band(title_h*s);
    let (row_a,nodes_a) = panel_row(f,inner,s,true);
    let (row_b,nodes_b) = panel_row(f,inner,s,true);
    field_nodes[index_a] = nodes_a.map(Some);
    field_nodes[index_b] = nodes_b.map(Some);
    let status=f.band(60.0*s);
    let box_node=f.node(Container::column().width(width).height(364.0*s).gap(10.0*s).pad(18.0*s,12.0*s),
        &[title,row_a,row_b,status]);
    (box_node,title,status)
}

/// Physical-pixel coordinates. Both input and painter call this function.
pub fn layout(width: u32, height: u32, ui: &AssetMappingUi) -> Layout {
    let (w,h)=(width.max(1) as f32,height.max(1) as f32);
    // Scale the *content* as well as the geometry on high-DPI displays.  The
    // original 1.17 cap left 1440p/4K text physically tiny on screen.
    // Compact 16% further than the previous 0.95 scale at desktop sizes.
    // Keep a readable minimum on small windows instead of shrinking text into
    // illegibility; the existing Taffy scroller handles constrained heights.
    let s=((w/1500.0).min(h/850.0).clamp(0.80,1.72)*0.80).clamp(0.78,1.38);
    let mw=(w-26.0*s).max(1.0).min(1200.0*s);
    let gap=16.0*s;
    let wide = (mw-gap)*0.5 >= 470.0*s;
    let col_w=if wide {(mw-gap)*0.5} else {mw};
    let header_h=99.0*s;
    let footer_h=110.0*s;
    let section_h=364.0*s;
    let override_h=286.0*s;
    let feedback_h=54.0*s;
    let content_h=if wide { section_h+gap+override_h+gap+feedback_h }
        else { section_h*2.0+gap*3.0+override_h+feedback_h };
    let view_h=(h-header_h-footer_h).max(30.0);
    let scroll_max=(content_h-view_h).max(0.0);
    let scroll=ui.scroll.clamp(0.0,scroll_max);
    let mut f=Flex::new();
    let title=f.band(50.0*s);
    let subtitle=f.band(33.0*s);
    let header=f.node(Container::column().width(mw).height(header_h).gap(1.0*s).pad(0.0,6.0*s),&[title,subtitle]);
    let mut field_nodes=[[None;4];6];
    let (jka,jka_title,retail)=section(&mut f,col_w,s,39.0,0,1,&mut field_nodes);
    let (jk2,jk2_title,jk2_status)=section(&mut f,col_w,s,39.0,2,3,&mut field_nodes);
    let both = if wide {
        f.node(Container::row().width(mw).height(section_h).gap(gap), &[jka,jk2])
    } else {
        f.node(Container::column().width(mw).height(section_h*2.0+gap).gap(gap), &[jka,jk2])
    };
    let force_title=f.band(28.0*s);
    let (jka_force_row,jka_parts)=panel_row(&mut f,(mw-36.0*s).max(1.0),s,false);
    let (jk2_force_row,jk2_parts)=panel_row(&mut f,(mw-36.0*s).max(1.0),s,false);
    field_nodes[4]=jka_parts.map(Some);
    field_nodes[5]=jk2_parts.map(Some);
    let override_panel=f.node(Container::column().width(mw).height(override_h).gap(10.0*s).pad(18.0*s,12.0*s),
        &[force_title,jka_force_row,jk2_force_row]);
    let feedback=f.band(feedback_h);
    let content=f.node(Container::column().width(mw).height(content_h).gap(gap), &[both,override_panel,feedback]);
    let scroller=f.node(Container::column().width(mw).height(view_h), &[content]);
    let save=f.leaf(160.0*s,50.0*s);
    let restart=f.leaf(215.0*s,50.0*s);
    let back=f.leaf(if ui.allow_close { 185.0*s } else { 0.0 },50.0*s);
    let actions=if ui.allow_close {
        f.node(Container::row().width(mw).height(50.0*s).gap(12.0*s), &[save,restart,back])
    } else {
        f.node(Container::row().width(mw).height(50.0*s).gap(12.0*s), &[save,restart])
    };
    let footer=f.node(Container::column().width(mw).height(footer_h).gap(10.0*s).pad(0.0,14.0*s), &[actions]);
    let column=f.node(Container::column().width(mw).height(h), &[header,scroller,footer]);
    let root=f.node(Container::row().width(w).height(h).justify(JustifyContent::CENTER), &[column]);
    f.compute(root,w,h);
    let bounds=|node| f.bounds(node);
    let fields=std::array::from_fn(|i| {
        let nodes=field_nodes[i].map(|node|bounds(node.expect("asset mapping row")));
        FieldLayout { label:translated(nodes[0],scroll), edit:translated(nodes[1],scroll),
            browse:translated(nodes[2],scroll), status:translated(nodes[3],scroll) }
    });
    let mut result=Layout {
        header:bounds(header), title:bounds(title),subtitle:bounds(subtitle),
        viewport:bounds(scroller), jka:translated(bounds(jka),scroll), jk2:translated(bounds(jk2),scroll),
        override_panel:translated(bounds(override_panel),scroll),
        jka_title:translated(bounds(jka_title),scroll), jk2_title:translated(bounds(jk2_title),scroll),
        force_title:translated(bounds(force_title),scroll),retail:translated(bounds(retail),scroll),
        jk2_status:translated(bounds(jk2_status),scroll),feedback:translated(bounds(feedback),scroll),
        fields,footer:bounds(footer),save:bounds(save),restart:bounds(restart),back:bounds(back),
        scroll_max, scroll, scale:s, picker:None,
    };
    if ui.picker.is_some() { result.picker=Some(picker_layout(width,height,s)); }
    result
}
fn picker_layout(width:u32,height:u32,s:f32)->PickerLayout {
    let (w,h)=(width as f32,height as f32);
    let mw=(w-26.0*s).max(1.0).min(850.0*s);
    let mh=(h-35.0*s).max(1.0).min(650.0*s);
    let inner=(mw-36.0*s).max(1.0);
    let mut f=Flex::new();
    let title=f.band(48.0*s);
    let parent=f.leaf(44.0*s,44.0*s);
    let refresh=f.leaf(44.0*s,44.0*s);
    let address=f.leaf((inner-104.0*s).max(1.0),44.0*s);
    let address_row=f.node(Container::row().width(inner).height(44.0*s).gap(8.0*s), &[parent,refresh,address]);
    let error=f.band(27.0*s);
    let list=f.spring();
    let choose=f.leaf(170.0*s,44.0*s);
    let cancel=f.leaf(116.0*s,44.0*s);
    let footer=f.node(Container::row().width(inner).height(44.0*s).gap(10.0*s),&[choose,cancel]);
    let panel=f.node(Container::column().width(mw).height(mh).pad(18.0*s,15.0*s).gap(10.0*s),
        &[title,address_row,error,list,footer]);
    let outer=f.node(Container::column().width(w).height(h).align(AlignItems::CENTER)
        .justify(JustifyContent::CENTER), &[panel]);
    f.compute(outer,w,h);
    PickerLayout {panel:f.bounds(panel),title:f.bounds(title),address:f.bounds(address),
        parent:f.bounds(parent),refresh:f.bounds(refresh),choose:f.bounds(choose),cancel:f.bounds(cancel),
        list:f.bounds(list),error:f.bounds(error),row_h:39.0*s }
}

impl Layout {
    pub fn hit(&self, pointer:[f32;2],ui:&AssetMappingUi)->Option<Hit> {
        if let (Some(p),Some(picker))=(self.picker,&ui.picker) {
            for (b,hit) in [(p.address,Hit::Address),(p.parent,Hit::Parent),
                (p.refresh,Hit::Refresh),(p.choose,Hit::Choose),(p.cancel,Hit::Cancel)] {
                if flex::contains(b,pointer) {return Some(hit);}
            }
            if flex::contains(p.list,pointer) {
                let ix=picker.scroll + ((pointer[1]-p.list[1])/p.row_h).floor().max(0.0) as usize;
                if ix<picker.entries.len() && ix-picker.scroll<self.picker_visible_rows() {return Some(Hit::Entry(ix));}
            }
            return None; // modal blocks main screen
        }
        for (b,hit) in [(self.save,Hit::Save),(self.restart,Hit::Restart),(self.back,Hit::Back)] {
            if flex::contains(b,pointer) && (hit!=Hit::Back || ui.allow_close) {return Some(hit);}
        }
        if flex::contains(self.viewport,pointer) {
            for (i,field) in self.fields.iter().enumerate() {
                if flex::contains(field.edit,pointer) {return Some(Hit::Edit(i));}
                if i<4 && flex::contains(field.browse,pointer) {return Some(Hit::Browse(i));}
            }
        }
        None
    }
    pub fn picker_visible_rows(&self)->usize {
        self.picker.map_or(0,|p|(p.list[3]/p.row_h).floor().max(1.0) as usize)
    }
}

/// Path fields intentionally use the native monospaced fallback glyphs. The
/// painter and winit pointer mapping therefore share exact character advances,
/// including on the no-PK3 asset recovery screen.
pub(crate) fn edit_capacity(width:f32, scale:f32)->usize {
    ((width-24.0*scale).max(0.0)/(9.0*scale)).floor().max(1.0) as usize
}
fn byte_at_char(value:&str, char_index:usize)->usize {
    value.char_indices().nth(char_index).map_or(value.len(),|(byte,_)|byte)
}
pub(crate) fn caret_from_x(value:&str, start:usize, b:Bounds, scale:f32, x:f32)->usize {
    let capacity=edit_capacity(b[2],scale);
    let start=start.min(value.chars().count().saturating_sub(capacity));
    let shown=value.chars().count().saturating_sub(start).min(capacity);
    let offset=((x-(b[0]+10.0*scale))/(9.0*scale)+0.5).floor()
        .max(0.0).min(shown as f32) as usize;
    byte_at_char(value,start+offset)
}
#[allow(clippy::too_many_arguments)]
fn edit_contents(out:&mut Vec<UiVertex>,font:&UiText<'_>,b:Bounds,value:&str,
    active:bool, caret:usize, anchor:Option<usize>, start:usize,w:u32,h:u32,s:f32) {
    let px=18.0*s;
    let capacity=edit_capacity(b[2],s);
    let total=value.chars().count();
    let first=if active {start.min(total.saturating_sub(capacity))}
        else {total.saturating_sub(capacity)};
    let last=(first+capacity).min(total);
    let begin=byte_at_char(value,first);
    let end=byte_at_char(value,last);
    let x=b[0]+10.0*s;
    let advance=9.0*s;
    if active {
        if let Some(anchor)=anchor.filter(|a|*a!=caret) {
            let sel_a=value[..anchor.min(value.len())].chars().count();
            let sel_b=value[..caret.min(value.len())].chars().count();
            let left=sel_a.min(sel_b).max(first);
            let right=sel_a.max(sel_b).min(last);
            if right>left {
                fill(out,[x+(left-first) as f32*advance,b[1]+4.0*s,
                    (right-left) as f32*advance,b[3]-8.0*s],
                    [0.13,0.37,0.53,0.94],w,h);
            }
        }
    }
    font.draw(out,&value[begin..end],x,font.baseline_in(b,px),px,WHITE);
    if active {
        let caret_char=value[..caret.min(value.len())].chars().count();
        if caret_char>=first && caret_char<=last {
            let cursor=(x+(caret_char-first) as f32*advance).min(b[0]+b[2]-9.0*s);
            rect(out,cursor,b[1]+8.0*s,1.5*s,b[3]-16.0*s,CYAN,w,h);
        }
    }
}
fn navigation_icon(out:&mut Vec<UiVertex>,b:Bounds,refresh:bool,hover:bool,w:u32,h:u32,s:f32) {
    fill(out,b,if hover {[0.12,0.30,0.39,0.98]}else{[0.055,0.11,0.16,0.98]},w,h);
    rect_outline(out,b[0],b[1],b[2],b[3],1.0,
        if hover{CYAN}else{[0.18,0.37,0.47,0.8]},w,h);
    let cx=b[0]+b[2]*0.5;
    let cy=b[1]+b[3]*0.5;
    let c=if hover{WHITE}else{CYAN};
    let thick=(2.6*s).max(1.7);
    if refresh {
        // Familiar clockwise refresh glyph with an arrowhead.
        let radius=10.0*s;
        let steps=18;
        let mut previous=None;
        for i in 0..=steps {
            let angle=(-140.0+290.0*(i as f32/steps as f32)).to_radians();
            let pos=[cx+radius*angle.cos(),cy+radius*angle.sin()];
            if let Some(last)=previous {neon_stroke(out,last,pos,thick,c,w,h);}
            previous=Some(pos);
        }
        let tip=[cx+radius*150.0_f32.to_radians().cos(),
            cy+radius*150.0_f32.to_radians().sin()];
        neon_stroke(out,tip,[tip[0]+8.0*s,tip[1]+1.0*s],thick,c,w,h);
        neon_stroke(out,tip,[tip[0]+2.0*s,tip[1]+8.0*s],thick,c,w,h);
    } else {
        // Standard "up one folder" icon, not a text PARENT button.
        neon_stroke(out,[cx,cy-11.0*s],[cx,cy+10.0*s],thick,c,w,h);
        neon_stroke(out,[cx,cy-11.0*s],[cx-9.0*s,cy-2.0*s],thick,c,w,h);
        neon_stroke(out,[cx,cy-11.0*s],[cx+9.0*s,cy-2.0*s],thick,c,w,h);
    }
}
fn entry_icon(out:&mut Vec<UiVertex>,x:f32,y:f32,folder:bool,s:f32,w:u32,h:u32) {
    if folder {
        fill(out,[x+1.0*s,y+7.0*s,23.0*s,15.0*s],[0.18,0.54,0.72,1.0],w,h);
        fill(out,[x+1.0*s,y+3.0*s,11.0*s,7.0*s],[0.18,0.54,0.72,1.0],w,h);
        rect_outline(out,x+1.0*s,y+7.0*s,23.0*s,15.0*s,1.0,CYAN,w,h);
    } else {
        fill(out,[x+4.0*s,y+2.0*s,17.0*s,22.0*s],[0.16,0.23,0.30,1.0],w,h);
        rect_outline(out,x+4.0*s,y+2.0*s,17.0*s,22.0*s,1.0,DIM,w,h);
        rect(out,x+8.0*s,y+11.0*s,9.0*s,1.0,DIM,w,h);
        rect(out,x+8.0*s,y+16.0*s,9.0*s,1.0,DIM,w,h);
    }
}
fn fill(out:&mut Vec<UiVertex>,b:Bounds,c:[f32;4],w:u32,h:u32){rect(out,b[0],b[1],b[2],b[3],c,w,h);}
fn frame(out:&mut Vec<UiVertex>,b:Bounds,w:u32,h:u32){
    fill(out,b,[0.028,0.050,0.073,0.97],w,h);
    rect_outline(out,b[0],b[1],b[2],b[3],1.0,[0.16,0.31,0.42,0.85],w,h);
}
fn fit(t:&UiText<'_>,s:&str,px:f32,max:f32)->String {
    if t.width(s,px)<=max {return s.to_owned();}
    let mut out=String::new();
    // Tail paths rather than displaying a useless C:\Games\... prefix.
    let suffix="...";
    for ch in s.chars().rev() {
        let candidate=format!("{suffix}{ch}{out}");
        if t.width(&candidate,px)>max {break;}
        out.insert(0,ch);
    }
    format!("{suffix}{out}")
}
fn line(out:&mut Vec<UiVertex>,font:&UiText<'_>,b:Bounds,s:&str,px:f32,c:[f32;4]){
    if b[2]<2.0{return;}
    let value=fit(font,s,px,b[2]-5.0);
    font.draw(out,&value,b[0],font.baseline_in(b,px),px,c);
}
// These are the same built-in tube letterforms used by the neon main menu,
// so the recovery title still has an authored vector font with no PK3s present.
fn neon_stroke(out:&mut Vec<UiVertex>, a:[f32;2], b:[f32;2], thick:f32,
    color:[f32;4],w:u32,h:u32) {
    let dx=b[0]-a[0];let dy=b[1]-a[1];
    let distance=(dx*dx+dy*dy).sqrt();
    if distance<0.0001 {return;}
    let nx=-dy/distance*thick*0.5;
    let ny=dx/distance*thick*0.5;
    let point=|x:f32,y:f32|UiVertex {
        position:[x/w.max(1) as f32*2.0-1.0,1.0-y/h.max(1) as f32*2.0],
        uv:[0.0,0.0],color:UiVertex::pack_color(color),textured:0.0,
    };
    let v0=point(a[0]+nx,a[1]+ny);
    let v1=point(a[0]-nx,a[1]-ny);
    let v2=point(b[0]-nx,b[1]-ny);
    let v3=point(b[0]+nx,b[1]+ny);
    out.extend_from_slice(&[v0,v1,v2,v0,v2,v3]);
}
fn neon_title(out:&mut Vec<UiVertex>, text:&str, bounds:Bounds, s:f32,w:u32,h:u32) {
    use crate::neon_menu::font::{self as neon,Prim};
    let placed=neon::place(text);
    let Some(last)=placed.last()else{return;};
    let advance=last.x+last.glyph.width;
    let height=31.0*s;
    let scale=(bounds[2]/(advance*height).max(1.0)).min(1.0)*height;
    let y=bounds[1]+(bounds[3]-scale)*0.5;
    let thick=(2.0*s).max(1.2);
    for glyph in &placed {
        let map=|pos:[f32;2]|[bounds[0]+(glyph.x+pos[0])*scale,y+(1.0-pos[1])*scale];
        for prim in glyph.glyph.prims.iter().copied() {
            match prim {
                Prim::Line{a,b}=>neon_stroke(out,map(a),map(b),thick,CYAN,w,h),
                Prim::Arc{c,r,start,sweep}=>{
                    let steps=(sweep.abs()/10.0).ceil().max(2.0) as usize;
                    let mut old=None;
                    for i in 0..=steps {
                        let theta=(start+sweep*i as f32/steps as f32).to_radians();
                        let pos=map([c[0]+r*theta.cos(),c[1]+r*theta.sin()]);
                        if let Some(prev)=old {neon_stroke(out,prev,pos,thick,CYAN,w,h);}
                        old=Some(pos);
                    }
                }
            }
        }
    }
}
fn button(out:&mut Vec<UiVertex>,font:&UiText<'_>,b:Bounds,name:&str,hover:bool,primary:bool,w:u32,h:u32,s:f32) {
    fill(out,b,if hover {[0.12,0.30,0.39,0.98]} else if primary {[0.07,0.23,0.32,0.98]} else {[0.055,0.11,0.16,0.98]},w,h);
    rect_outline(out,b[0],b[1],b[2],b[3],1.0,if hover {CYAN}else{[0.18,0.37,0.47,0.8]},w,h);
    let px=18.0*s;
    let tw=font.width(name,px);
    line(out,font,[b[0]+((b[2]-tw)*0.5).max(5.0),b[1],b[2]-10.0,b[3]],name,px,if hover{WHITE}else{CYAN});
}
fn status_line(out:&mut Vec<UiVertex>,font:&UiText<'_>,b:Bounds,message:&str,
    color:[f32;4],w:u32,h:u32,s:f32) {
    let background=if color==GREEN {[0.039,0.145,0.087,0.92]}
        else if color==RED {[0.21,0.065,0.074,0.92]}
        else if color==AMBER {[0.18,0.13,0.06,0.94]}
        else {[0.067,0.09,0.12,0.94]};
    fill(out,b,background,w,h);
    rect(out,b[0],b[1],4.0*s,b[3],color,w,h);
    line(out,font,[b[0]+12.0*s,b[1],b[2]-17.0*s,b[3]],message,16.0*s,color);
}
pub(crate) fn build(out:&mut Vec<UiVertex>,ui:&UiSnapshot,w:u32,h:u32) {
    let Some(state)=ui.asset_mapping.as_ref() else{return;};
    let l=layout(w,h,state);
    let font=UiText::new(ui,None,w,h);
    let s=l.scale;
    fill(out,[0.0,0.0,w as f32,h as f32],[0.018,0.032,0.050,1.0],w,h);
    fill(out,[0.0,0.0,w as f32,4.0*s],CYAN,w,h);
    neon_title(out,"ASSET MAPPING",l.title,s,w,h);
    line(out,&font,l.subtitle,if state.allow_close {"DINURDOJK / FILESYSTEM SETTINGS  -  CHANGES TAKE EFFECT AFTER RESTART"}
        else {"JEDI ACADEMY RETAIL ASSETS NOT DETECTED  -  LOCATE YOUR GAME ROOT"},18.0*s,CYAN);
    for (panel,heading,name) in [(l.jka,l.jka_title,"JEDI ACADEMY  /  REQUIRED"),
        (l.jk2,l.jk2_title,"JEDI OUTCAST  /  OPTIONAL"),
        (l.override_panel,l.force_title,"GAME ASSET OVERRIDES  /  OPTIONAL")] {
        // The scroll viewport clips each panel; do not paint beyond the footer.
        let top=panel[1].max(l.viewport[1]);
        let bottom=(panel[1]+panel[3]).min(l.viewport[1]+l.viewport[3]);
        if bottom>top {
            fill(out,[panel[0],top,panel[2],bottom-top],[0.028,0.050,0.073,0.97],w,h);
            if visible(heading,l.viewport){line(out,&font,heading,name,21.0*s,WHITE);}
            rect(out,panel[0],top,1.0,bottom-top,[0.16,0.31,0.42,0.85],w,h);
            rect(out,panel[0]+panel[2]-1.0,top,1.0,bottom-top,[0.16,0.31,0.42,0.85],w,h);
            if panel[1]>=l.viewport[1]{rect(out,panel[0],panel[1],panel[2],1.0,[0.16,0.31,0.42,0.85],w,h);}
            if panel[1]+panel[3]<=l.viewport[1]+l.viewport[3] {
                rect(out,panel[0],panel[1]+panel[3]-1.0,panel[2],1.0,[0.16,0.31,0.42,0.85],w,h);
            }
        }
    }
    let names=["fs_basepath", "fs_cdpath", "fs_jk2basepath", "fs_jk2cdpath",
        "fs_forcegame", "fs_jk2forcegame"];
    let descriptions=["JKA game root (contains base/)","Optional retail JKA root (read-only)",
        "JK2 game root (contains base/)","Optional retail JK2 root (read-only)",
        "JKA override name under fs_basepath/ (not fs_game)",
        "JK2 override name under fs_jk2basepath/ (not fs_game)"];
    for (i,row) in l.fields.iter().enumerate() {
        if !visible(row.label,l.viewport) || !visible(row.edit,l.viewport) {continue;}
        let label=format!("{}   /   {}",names[i],descriptions[i]);
        line(out,&font,row.label,&label,17.0*s,DIM);
        let active=state.focus==Some(i) && state.picker.is_none();
        fill(out,row.edit,if active {[0.082,0.156,0.21,0.98]}else{[0.038,0.077,0.116,0.99]},w,h);
        rect_outline(out,row.edit[0],row.edit[1],row.edit[2],row.edit[3],1.0,
            if active{CYAN}else{[0.16,0.28,0.35,1.0]},w,h);
        if state.values[i].is_empty() && !active {
            line(out,&font,[row.edit[0]+10.0*s,row.edit[1],row.edit[2]-20.0*s,row.edit[3]],match i {
                0=>"Default / EXE directory",
                2=>"Not set / JK2 disabled",
                4|5=>"Empty = disabled",
                _=>"Not configured",
            },18.0*s,DIM);
        } else {
            let mono=UiText::legacy_only(None,w,h);
            edit_contents(out,&mono,row.edit,&state.values[i],active,state.caret,
                if active {state.selection_anchor}else{None},state.text_start,w,h,s);
        }
        if i<4 {
            let hover=l.hit(state.pointer,state)==Some(Hit::Browse(i));
            button(out,&font,row.browse,"BROWSE",hover,false,w,h,s);
            if visible(row.status,l.viewport) {
                let (status,color)=match state.valid[i] {
                    Some(true)=>("FOUND  /  base/ directory exists",GREEN),
                    Some(false)=>("NOT FOUND  /  base/ directory is missing",RED),
                    None if i==0 || i==2 => ("NOT SET  /  choose a game root",RED),
                    None => ("NOT SET  /  optional CD fallback disabled",DIM),
                };
                status_line(out,&font,row.status,status,color,w,h,s);
            }
        } else if visible(row.status,l.viewport) {
            let idx=i-4;
            let (status,color)=if state.force_invalid[idx] {
                ("INVALID NAME  /  use one folder name",RED)
            } else if state.values[i].trim().is_empty() {
                ("DISABLED  /  no forced asset override",DIM)
            } else if i==5 && state.values[2].trim().is_empty() {
                ("INACTIVE  /  configure fs_jk2basepath first",AMBER)
            } else if state.force_status[idx]==Some(true) {
                ("FOUND  /  game-root override directory exists",GREEN)
            } else {
                ("NEW  /  override folder will be created on save",AMBER)
            };
            status_line(out,&font,row.status,status,color,w,h,s);
        }
    }
    // Both games get the same two-line presentation: one colored readiness
    // summary and one diagnostic line.  Individual PK3 names appear only if
    // missing, rather than listing every healthy JKA archive but not JK2's.
    if visible(l.retail,l.viewport) {
        let found=state.retail.iter().filter(|present|**present).count();
        let ready=found==state.retail.len();
        status_line(out,&font,[l.retail[0],l.retail[1],l.retail[2],27.0*s],
            if ready {"JKA RETAIL  /  READY"} else {"JKA RETAIL  /  MISSING ASSETS"},
            if ready {GREEN}else{RED},w,h,s);
        let detail=if ready {
            "All four required retail archives detected".to_owned()
        } else {
            let missing=(0..state.retail.len()).filter(|&i|!state.retail[i])
                .map(|i|format!("assets{i}.pk3")).collect::<Vec<_>>().join(", ");
            format!("Missing: {missing}")
        };
        line(out,&font,[l.retail[0]+12.0*s,l.retail[1]+31.0*s,
            l.retail[2]-20.0*s,26.0*s],&detail,15.0*s,if ready {DIM}else{RED});
    }
    if visible(l.jk2_status,l.viewport) {
        let v102=state.jk2_status[0]=="READY";
        let v104=state.jk2_status[1]=="READY";
        let ready=v102&&v104;
        status_line(out,&font,[l.jk2_status[0],l.jk2_status[1],l.jk2_status[2],27.0*s],
            if ready {"JK2 RETAIL  /  READY"} else if v102||v104 {
                "JK2 RETAIL  /  PARTIAL"
            } else {"JK2 RETAIL  /  MISSING ASSETS"},
            if ready {GREEN} else if v102||v104 {AMBER} else {RED},w,h,s);
        let detail=if ready {
            "Both JK2 1.02 and 1.04 asset sets detected".to_owned()
        } else {
            format!("1.02: {}   /   1.04: {}",state.jk2_status[0],state.jk2_status[1])
        };
        line(out,&font,[l.jk2_status[0]+12.0*s,l.jk2_status[1]+31.0*s,
            l.jk2_status[2]-20.0*s,26.0*s],&detail,15.0*s,
            if ready {DIM}else if v102||v104 {AMBER}else{RED});
    }
    if visible(l.feedback,l.viewport) {
        if let Some(message)=&state.feedback {line(out,&font,l.feedback,message,17.0*s,AMBER);}
        else {line(out,&font,l.feedback,"Game root paths contain base/; no files are moved or copied.",17.0*s,DIM);}
    }
    // Stable footer stays clickable even when content scrolls.
    fill(out,[0.0,l.footer[1],w as f32,(h as f32-l.footer[1]).max(0.0)],
        [0.023,0.040,0.060,1.0],w,h);
    rect(out,0.0,l.footer[1],w as f32,1.0,[0.15,0.33,0.43,1.0],w,h);
    for (rect,hit,text,primary) in [(l.save,Hit::Save,"SAVE PATHS",false),
        (l.restart,Hit::Restart,"SAVE & RESTART",true),(l.back,Hit::Back,"BACK TO MENU",false)] {
        if hit!=Hit::Back || state.allow_close {
            button(out,&font,rect,text,l.hit(state.pointer,state)==Some(hit),primary,w,h,s);
        }
    }
    line(out,&font,[l.footer[0],l.footer[1]+80.0*s,l.footer[2],19.0*s],
        "Saved to DinurdoJK/DinurdoJK-paths.ini  /  No asset migration   /   ` toggles console",15.0*s,DIM);
    if l.scroll_max>0.0 {
        let b=[l.viewport[0]+l.viewport[2]-4.0*s,l.viewport[1],3.0*s,l.viewport[3]];
        fill(out,b,[0.16,0.26,0.35,0.8],w,h);
        let thumb=(l.viewport[3]*l.viewport[3]/(l.viewport[3]+l.scroll_max)).max(18.0*s);
        let y=b[1]+(l.scroll/l.scroll_max)*(b[3]-thumb).max(0.0);
        fill(out,[b[0],y,b[2],thumb],CYAN,w,h);
    }
    if let (Some(p),Some(data))=(l.picker,&state.picker) {
        fill(out,[0.0,0.0,w as f32,h as f32],[0.012,0.017,0.027,0.94],w,h);
        frame(out,p.panel,w,h);
        let title=match data.target {
            0=>"JEDI ACADEMY / GAME ROOT",
            1=>"JEDI ACADEMY / RETAIL ROOT",
            2=>"JEDI OUTCAST / GAME ROOT",
            3=>"JEDI OUTCAST / RETAIL ROOT",
            _=>"SELECT FOLDER",
        };
        line(out,&font,p.title,title,25.0*s,WHITE);
        for (bounds,hit,refresh) in [(p.parent,Hit::Parent,false),(p.refresh,Hit::Refresh,true)] {
            navigation_icon(out,bounds,refresh,l.hit(state.pointer,state)==Some(hit),w,h,s);
        }
        fill(out,p.address,[0.035,0.082,0.12,1.0],w,h);
        rect_outline(out,p.address[0],p.address[1],p.address[2],p.address[3],1.0,
            if state.focus==Some(6){CYAN}else{DIM},w,h);
        let mono=UiText::legacy_only(None,w,h);
        edit_contents(out,&mono,p.address,&data.address,state.focus==Some(6),state.caret,
            if state.focus==Some(6){state.selection_anchor}else{None},state.text_start,w,h,s);
        for (bounds,hit,name) in [(p.choose,Hit::Choose,"SELECT FOLDER"),(p.cancel,Hit::Cancel,"CANCEL")] {
            button(out,&font,bounds,name,l.hit(state.pointer,state)==Some(hit),hit==Hit::Choose,w,h,s);
        }
        if let Some(error)=&data.error {
            line(out,&font,p.error,error,16.0*s,RED);
        } else {
            let folders=data.entries.iter().filter(|entry|entry.is_dir).count();
            let files=data.entries.len()-folders;
            let summary=format!("{folders} folders  /  {files} files   -   Up arrow: parent folder   -   Enter: open typed path");
            line(out,&font,p.error,&summary,16.0*s,DIM);
        }
        frame(out,p.list,w,h);
        let count=l.picker_visible_rows();
        for (offset,entry) in data.entries.iter().skip(data.scroll).take(count).enumerate() {
            let b=[p.list[0]+9.0*s,p.list[1]+offset as f32*p.row_h+2.0*s,p.list[2]-18.0*s,p.row_h-2.0*s];
            if entry.is_dir && l.hit(state.pointer,state)==Some(Hit::Entry(data.scroll+offset)) {
                fill(out,b,[0.1,0.24,0.32,0.94],w,h);
            } else if offset%2==1 {
                fill(out,b,[0.044,0.074,0.100,0.65],w,h);
            }
            entry_icon(out,b[0]+7.0*s,b[1]+4.0*s,entry.is_dir,s,w,h);
            let kind=if entry.is_dir {"FOLDER"}else{"FILE"};
            let kind_width=font.width(kind,14.0*s);
            let kind_x=b[0]+b[2]-kind_width-11.0*s;
            let text_x=b[0]+39.0*s;
            line(out,&font,[text_x,b[1],(kind_x-text_x-12.0*s).max(0.0),b[3]],
                &entry.name,17.0*s,if entry.is_dir{WHITE}else{DIM});
            line(out,&font,[kind_x,b[1],kind_width+3.0*s,b[3]],kind,14.0*s,DIM);
        }
        if data.entries.is_empty() && data.error.is_none() {
            line(out,&font,[p.list[0]+15.0*s,p.list[1]+15.0*s,p.list[2]-30.0*s,36.0*s],
                "This folder is empty",17.0*s,DIM);
        }
        if data.entries.len()>count {
            let b=[p.list[0]+p.list[2]-5.0*s,p.list[1],3.0*s,p.list[3]];
            fill(out,b,[0.16,0.26,0.35,0.8],w,h);
            let thumb=(b[3]*count as f32/data.entries.len() as f32).max(18.0*s).min(b[3]);
            let max_scroll=data.entries.len().saturating_sub(count).max(1);
            let y=b[1]+data.scroll as f32/max_scroll as f32*(b[3]-thumb);
            fill(out,[b[0],y,b[2],thumb],CYAN,w,h);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn taffy_controls_and_hitboxes_agree() {
        for (w,h) in [(640,480),(900,600),(1920,1080),(2560,1440)] {
            let ui=AssetMappingUi {allow_close:true,..Default::default()};
            let l=layout(w,h,&ui);
            let mid=|b:Bounds|[b[0]+b[2]*0.5,b[1]+b[3]*0.5];
            assert_eq!(l.hit(mid(l.save),&ui),Some(Hit::Save));
            assert_eq!(l.hit(mid(l.restart),&ui),Some(Hit::Restart));
            assert_eq!(l.hit(mid(l.back),&ui),Some(Hit::Back));
            assert!(l.scroll_max>=0.0);
            assert!(l.fields[5].edit[1]>l.fields[4].edit[1]);
            let mut at_bottom=ui.clone();
            at_bottom.scroll=l.scroll_max;
            let bottom_layout=layout(w,h,&at_bottom);
            let target=bottom_layout.fields[5].edit;
            if visible(target,bottom_layout.viewport) {
                assert_eq!(bottom_layout.hit(mid(target),&at_bottom),Some(Hit::Edit(5)));
            }
        }
    }
}

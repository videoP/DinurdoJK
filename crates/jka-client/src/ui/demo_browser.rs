//! Play Demo workspace: native WGPU vertices and native winit hit testing.
//! Taffy owns the geometry; no egui widgets, painter or scroll areas.
use super::{flex::{self, Container, Flex}, rect, rect_outline, text::UiText, UiSnapshot, UiVertex};
use taffy::prelude::AlignItems;

pub(crate) const ROW: f32 = 39.0;
const WHITE: [f32; 4] = [0.91, 0.95, 1.0, 1.0];
const DIM: [f32; 4] = [0.57, 0.68, 0.78, 1.0];
const CYAN: [f32; 4] = [0.38, 0.79, 1.0, 1.0];
const GREEN: [f32; 4] = [0.43, 0.86, 0.58, 1.0];
const RED: [f32; 4] = [1.0, 0.39, 0.43, 1.0];
const AMBER: [f32; 4] = [1.0, 0.73, 0.38, 1.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind { Heading, Value, Entry, Note }
#[derive(Clone, Debug)]
pub struct BrowserLine {
    pub kind: LineKind,
    pub title: String,
    pub subtitle: String,
    pub right: String,
    pub present: Option<bool>,
    pub seek_ms: Option<i32>,
}
impl BrowserLine {
    pub fn new(kind: LineKind, title: impl Into<String>, subtitle: impl Into<String>, right: impl Into<String>) -> Self {
        Self { kind, title: title.into(), subtitle: subtitle.into(), right: right.into(), present: None, seek_ms: None }
    }
}

#[derive(Clone, Debug, Default)]
pub struct DemoBrowserUi {
    /// True for JK2 (.dm_15 / .dm_16), false for JKA (.dm_26).
    pub jk2: bool,
    pub folders: Vec<String>,
    pub folder: String,
    pub folder_open: bool,
    pub folder_scroll: usize,
    pub names: Vec<String>,
    pub selected: usize,
    pub tab: usize,
    pub list_scroll: usize,
    pub detail_scroll: usize,
    pub missing_only: bool,
    pub console_filter: usize,
    pub title: String,
    pub summary: String,
    pub status: String,
    pub lines: Vec<BrowserLine>,
    pub pointer: [f32; 2],
}

pub(crate) const TABS: [&str; 5] = ["OVERVIEW", "PLAYERS", "ROUNDS / DUELS", "CONSOLE", "REFERENCED ASSETS"];
pub(crate) const FILTERS: [&str; 4] = ["ALL", "PRINT", "CHAT", "EVENTS"];

#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub outer: [f32; 4],
    pub game_tabs: [f32; 4],
    pub folder: [f32; 4],
    pub list: [f32; 4],
    pub tabs: [f32; 4],
    pub content: [f32; 4],
    pub actions: [f32; 4],
    pub play: [f32; 4],
    pub refresh: [f32; 4],
    pub missing: [f32; 4],
    pub filters: [f32; 4],
}
impl Layout {
    pub fn game_tab_rect(self, index: usize) -> [f32; 4] {
        let [x, y, w, h] = self.game_tabs;
        [x + index as f32 * w/2.0, y, w/2.0, h]
    }
    pub fn list_rows(self) -> [f32; 4] {
        let [x,y,w,h] = self.list;
        [x+14.0, y+49.0, (w-28.0).max(0.0), (h-60.0).max(0.0)]
    }
    pub fn detail_rows(self) -> [f32; 4] {
        let [x,y,w,h] = self.content;
        [x+16.0, y+63.0, (w-32.0).max(0.0), (h-78.0).max(0.0)]
    }
    pub fn tab_rect(self, index: usize) -> [f32; 4] {
        let [x,y,w,h] = self.tabs;
        let part = w / TABS.len() as f32;
        [x + index as f32 * part, y, part, h]
    }
    pub fn filter_rect(self, index: usize) -> [f32; 4] {
        let [x,y,w,h] = self.filters;
        [x + index as f32 * (w/4.0), y, w/4.0, h]
    }
    pub fn folder_item(self, index: usize) -> [f32; 4] {
        [self.folder[0], self.folder[1] + self.folder[3] + index as f32 * 34.0,
            self.folder[2], 34.0]
    }
    pub fn list_visible(self) -> usize { (self.list_rows()[3]/ROW).floor().max(1.0) as usize }
    pub fn detail_visible(self) -> usize { (self.detail_rows()[3]/ROW).floor().max(1.0) as usize }
}
fn translate(mut b: [f32; 4], x: f32, y: f32) -> [f32; 4] { b[0]+=x; b[1]+=y; b }

/// Physical-pixel layout. Exactly the same function is used by WGPU and input.
pub fn layout(width: u32, height: u32) -> Layout {
    let (w,h) = (width as f32, height as f32);
    let mx = (w*0.057).clamp(16.0, 110.0);
    let my = (h*0.086).clamp(58.0, 118.0);
    let total_w = (w-2.0*mx).max(350.0);
    let total_h = (h-my-82.0).max(240.0);
    let sidebar_w = (total_w * 0.275).clamp(220.0, 360.0).min(total_w*0.44);
    let mut flex = Flex::new();
    let folder = flex.band(156.0);
    let list = flex.spring();
    let sidebar = flex.node(Container::column().width(sidebar_w).height(total_h).gap(7.0).align(AlignItems::STRETCH), &[folder, list]);
    let heading = flex.band(72.0);
    let tabs = flex.band(50.0);
    let content = flex.spring();
    let actions = flex.band(64.0);
    let inspector = flex.node(Container::column().height(total_h).gap(6.0).align(AlignItems::STRETCH), &[heading,tabs,content,actions]);
    flex.set_grow(inspector,1.0);
    let root = flex.node(Container::row().width(total_w).height(total_h).gap(13.0).align(AlignItems::STRETCH), &[sidebar,inspector]);
    flex.compute(root,total_w,total_h);
    let at = |node| translate(flex.bounds(node),mx,my);
    let folder_box = at(folder);
    let content_box = at(content);
    let action_box = at(actions);
    Layout {
        outer: [mx,my,total_w,total_h],
        game_tabs: [mx+14.0, my+51.0, (sidebar_w-28.0).max(0.0), 32.0],
        folder: [folder_box[0]+14.0, folder_box[1]+111.0, (folder_box[2]-28.0).max(0.0),38.0],
        list: at(list), tabs: at(tabs), content: content_box, actions: action_box,
        play: [action_box[0]+10.0,action_box[1]+10.0,162.0,42.0],
        refresh: [action_box[0]+185.0,action_box[1]+12.0,118.0,38.0],
        missing: [content_box[0]+content_box[2]-214.0,content_box[1]+10.0,192.0,36.0],
        filters: [content_box[0]+content_box[2]-362.0,content_box[1]+10.0,340.0,34.0],
    }
}

fn box_fill(out: &mut Vec<UiVertex>, bounds: [f32;4], color:[f32;4], w:u32,h:u32) {
    rect(out,bounds[0],bounds[1],bounds[2],bounds[3],color,w,h);
}
fn frame(out:&mut Vec<UiVertex>, b:[f32;4], w:u32,h:u32) {
    box_fill(out,b,[0.025,0.049,0.074,0.88],w,h);
    rect_outline(out,b[0],b[1],b[2],b[3],1.0,[0.19,0.35,0.45,0.70],w,h);
}
fn line(out:&mut Vec<UiVertex>,font:&UiText<'_>,b:[f32;4],s:&str,size:f32,c:[f32;4], w:u32,h:u32) {
    if b[2] <= 1.0 {return;}
    // No glyph geometry may escape its column; clip by measured width.
    let mut end=s.len();
    while end>0 && font.width(&s[..end],size)>b[2]-4.0 {
        end=s[..end].char_indices().last().map_or(0,|(i,_)|i);
    }
    let truncated=end<s.len();
    let mut display=s[..end].to_owned();
    if truncated {
        while !display.is_empty() && font.width(&(display.clone()+"..."),size)>b[2]-4.0 {display.pop();}
        display.push_str("...");
    }
    font.draw(out,&display,b[0],font.baseline_in(b,size),size,c);
    let _=(w,h);
}
fn button(out:&mut Vec<UiVertex>,font:&UiText<'_>,b:[f32;4],name:&str,active:bool,hover:bool,w:u32,h:u32) {
    box_fill(out,b,if active {[0.105,0.31,0.43,0.98]} else if hover {[0.11,0.20,0.27,0.96]} else {[0.055,0.105,0.15,0.95]},w,h);
    rect_outline(out,b[0],b[1],b[2],b[3],1.0,if active {CYAN} else {[0.22,0.35,0.44,0.7]},w,h);
    let px=(b[3]*0.42).clamp(14.0,20.0);
    let tw=font.width(name,px);
    line(out,font,[b[0]+((b[2]-tw)*0.5).max(5.0),b[1],b[2]-9.0,b[3]],name,px,if active {WHITE}else{DIM},w,h);
}
fn scroll_marker(out:&mut Vec<UiVertex>,b:[f32;4],count:usize,first:usize,visible:usize,w:u32,h:u32) {
    if count<=visible{return;}
    let fraction=visible as f32/count as f32;
    let y=b[1]+b[3]*(first as f32/count as f32);
    box_fill(out,[b[0]+b[2]-5.0,b[1],3.0,b[3]],[0.17,0.25,0.32,0.7],w,h);
    box_fill(out,[b[0]+b[2]-5.0,y,3.0,(b[3]*fraction).max(16.0).min(b[3])],CYAN,w,h);
}

pub(crate) fn build(out:&mut Vec<UiVertex>, ui:&UiSnapshot,w:u32,h:u32) {
    let Some(browser)=ui.demo_browser.as_ref() else{return;};
    let l=layout(w,h);
    let font=UiText::new(ui,None,w,h);
    let hover=|b| flex::contains(b,browser.pointer);
    let scale=(h as f32/900.0).clamp(0.88,1.22);
    let large=(23.0*scale).max(22.0);
    let mid=(17.0*scale).max(16.0);
    let small=(14.0*scale).max(13.5);
    let micro=(12.5*scale).max(12.0);
    frame(out,l.outer,w,h);
    // Decorative chrome / direct information density: no nested cards or egui controls.
    let [x,y,_,_]=l.outer;
    line(out,&font,[x+26.0,y+12.0,420.0,35.0],"DEMO ARCHIVE",26.0*scale,WHITE,w,h);
    for (index,label) in ["JKA", "JK2"].iter().enumerate() {
        let b=l.game_tab_rect(index);
        button(out,&font,[b[0]+2.0,b[1],b[2]-4.0,b[3]],label,browser.jk2==(index==1),hover(b),w,h);
    }
    line(out,&font,[l.folder[0],l.folder[1]-27.0,250.0,25.0],"DEMO FOLDER",small,CYAN,w,h);
    button(out,&font,l.folder,&format!("{}  v",browser.folder),true,hover(l.folder),w,h);
    frame(out,l.list,w,h);
    line(out,&font,[l.list[0]+14.0,l.list[1]+12.0,l.list[2]-28.0,32.0],
        &format!("RECORDINGS  /  {}",browser.names.len()),mid,CYAN,w,h);
    let lr=l.list_rows();
    let lv=l.list_visible();
    let start=browser.list_scroll.min(browser.names.len().saturating_sub(lv));
    for (visible,name) in browser.names.iter().skip(start).take(lv).enumerate() {
        let index=start+visible;
        let b=[lr[0],lr[1]+visible as f32*ROW,lr[2]-7.0,ROW-2.0];
        if index==browser.selected || hover(b) {
            box_fill(out,b,if index==browser.selected {[0.095,0.25,0.36,0.97]}else{[0.11,0.17,0.24,0.95]},w,h);
        }
        if index==browser.selected {box_fill(out,[b[0],b[1]+4.0,3.0,b[3]-8.0],CYAN,w,h);}
        line(out,&font,[b[0]+11.0,b[1],b[2]-20.0,b[3]],name,mid,
            if index==browser.selected {WHITE}else{DIM},w,h);
    }
    scroll_marker(out,lr,browser.names.len(),start,lv,w,h);
    // Inspector title and segmented Taffy tabs.
    let right=l.tabs[0];
    line(out,&font,[right+12.0,l.tabs[1]-68.0,l.tabs[2]-24.0,35.0],&browser.title,large,WHITE,w,h);
    line(out,&font,[right+12.0,l.tabs[1]-32.0,l.tabs[2]-24.0,28.0],&browser.summary,small,DIM,w,h);
    for (i,label) in TABS.iter().enumerate() {
        let b=l.tab_rect(i);
        button(out,&font,[b[0]+2.0,b[1]+2.0,b[2]-4.0,b[3]-4.0],label,i==browser.tab,hover(b),w,h);
    }
    frame(out,l.content,w,h);
    let [cx,cy,cw,_]=l.content;
    let heading=if browser.jk2 {
        match browser.tab { 0=>"JK2 / OVERVIEW",1=>"JK2 / PLAYERS",2=>"JK2 / MATCH",3=>"JK2 / CONSOLE",_=>"JK2 / ASSETS" }
    } else {match browser.tab {0=>"RECORDING / OVERVIEW",1=>"ROSTER / IDENTITIES",2=>"MATCH / ROUNDS & DUELS",3=>"CONSOLE / EVENTS",_=>"REFERENCED ASSETS / LOCAL VFS"}};
    let heading_width = if browser.tab==3 { cw-380.0 } else if browser.tab==4 { cw-244.0 } else { cw-35.0 };
    line(out,&font,[cx+16.0,cy+12.0,heading_width.max(50.0),32.0],heading,mid,CYAN,w,h);
    if browser.tab==4 && !browser.jk2 {
        button(out,&font,l.missing,if browser.missing_only{"[x] MISSING ONLY"}else{"[ ] MISSING ONLY"},browser.missing_only,hover(l.missing),w,h);
    }
    if browser.tab==3 && !browser.jk2 {
        for (index,label) in FILTERS.iter().enumerate() {
            let b=l.filter_rect(index);
            button(out,&font,[b[0]+2.0,b[1],b[2]-4.0,b[3]],label,browser.console_filter==index,hover(b),w,h);
        }
    }
    let dr=l.detail_rows();
    let dv=l.detail_visible();
    let from=browser.detail_scroll.min(browser.lines.len().saturating_sub(dv));
    for (visible,item) in browser.lines.iter().skip(from).take(dv).enumerate() {
        let b=[dr[0],dr[1]+visible as f32*ROW,dr[2]-8.0,ROW-2.0];
        let color=match item.present {Some(true)=>GREEN,Some(false)=>RED,None=>WHITE};
        match item.kind {
            LineKind::Heading => {
                box_fill(out,b,[0.071,0.15,0.20,0.92],w,h);
                box_fill(out,[b[0]+1.0,b[1]+7.0,3.0,b[3]-14.0],CYAN,w,h);
                line(out,&font,[b[0]+13.0,b[1],b[2]-25.0,b[3]],&item.title,mid,CYAN,w,h);
            }
            LineKind::Note => {
                line(out,&font,[b[0]+8.0,b[1],b[2]-16.0,b[3]],&item.title,small,AMBER,w,h);
            }
            LineKind::Entry | LineKind::Value => {
                if visible%2==0 {box_fill(out,b,[0.055,0.095,0.135,0.72],w,h);}
                if item.seek_ms.is_some() && hover(b) {box_fill(out,b,[0.12,0.19,0.25,0.92],w,h);}
                if item.present.is_some() {box_fill(out,[b[0]+5.0,b[1]+14.0,6.0,9.0],color,w,h);}
                let left=if item.present.is_some(){19.0}else{10.0};
                let right_w=if item.right.is_empty(){0.0}else{(b[2]*0.25).clamp(90.0,250.0)};
                if item.subtitle.is_empty() {
                    line(out,&font,[b[0]+left,b[1],b[2]-left-right_w-9.0,b[3]],&item.title,mid,color,w,h);
                }else{
                    line(out,&font,[b[0]+left,b[1]+1.0,b[2]-left-right_w-9.0,20.0],&item.title,mid,color,w,h);
                    line(out,&font,[b[0]+left,b[1]+22.0,b[2]-left-right_w-9.0,14.0],&item.subtitle,micro,DIM,w,h);
                }
                if !item.right.is_empty(){line(out,&font,[b[0]+b[2]-right_w,b[1],right_w-8.0,b[3]],&item.right,micro,if item.present.is_some(){color}else{DIM},w,h);}
            }
        }
    }
    scroll_marker(out,dr,browser.lines.len(),from,dv,w,h);
    if browser.lines.is_empty() {line(out,&font,[dr[0]+7.0,dr[1],dr[2]-14.0,ROW],&browser.status,mid,DIM,w,h);}
    button(out,&font,l.play,"PLAY DEMO",true,hover(l.play),w,h);
    button(out,&font,l.refresh,"REFRESH",false,hover(l.refresh),w,h);
    // Keep the controls readable even when the inspector is narrow.
    let hint_width = (l.actions[0] + l.actions[2] - (l.refresh[0] + l.refresh[2]) - 17.0).max(0.0);
    if hint_width > 100.0 {
        line(out,&font,[l.refresh[0]+l.refresh[2]+12.0,l.actions[1],hint_width,l.actions[3]],
            if browser.tab==3 && !browser.jk2 && hint_width>235.0 {"CLICK MESSAGE: SEEK  ·  ESC: BACK"}
            else if hint_width>235.0 {"ENTER: PLAY  ·  ESC: BACK"}
            else {"ESC: BACK"},small,DIM,w,h);
    }
    // The folder chooser is an explicit native overlay, not an egui popup.
    if browser.folder_open {
        let b=[l.folder[0],l.folder[1]+l.folder[3],l.folder[2],browser.folders.len().min(12) as f32*34.0];
        frame(out,b,w,h);
        for (i,name) in browser.folders.iter().skip(browser.folder_scroll).take(12).enumerate() {
            let r=l.folder_item(i);
            if hover(r){box_fill(out,r,[0.12,0.24,0.32,0.98],w,h);}
            line(out,&font,[r[0]+10.0,r[1],r[2]-20.0,r[3]],name,mid,WHITE,w,h);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_taffy_panes_remain_in_bounds() {
        for (w,h) in [(800,600),(1280,720),(1649,928),(2560,1440)] {
            let l=layout(w,h);
            for b in [l.folder,l.game_tabs,l.list,l.tabs,l.content,l.actions,l.play,l.refresh] {
                assert!(b[0]>=0.0 && b[1]>=0.0 && b[2]>0.0 && b[3]>0.0,"{w}x{h} {b:?}");
                assert!(b[0]+b[2]<=w as f32+1.0 && b[1]+b[3]<=h as f32+1.0);
            }
            assert!(l.list[0]+l.list[2] < l.content[0]);
            assert!(l.tabs[1]+l.tabs[3] <= l.content[1]);
            assert!(l.content[1]+l.content[3] <= l.actions[1]);
            assert!(l.play[0]+l.play[2]<l.refresh[0]);
            assert!(l.refresh[0]+l.refresh[2]<=l.actions[0]+l.actions[2]);
        }
    }
}

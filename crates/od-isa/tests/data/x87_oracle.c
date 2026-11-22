
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <stdint.h>
static int hex(const char*s,unsigned char*o,int n){for(int i=0;i<n;i++){unsigned v;if(sscanf(s+2*i,"%2x",&v)!=1)return -1;o[i]=v;}return 0;}
static void ph(const unsigned char*b,int n){for(int i=0;i<n;i++)printf("%02x",b[i]);}
int main(){
  char op[32],xs[64],ys[64];unsigned fcw,sw0;
  while(scanf("%31s %x %x %63s %63s",op,&fcw,&sw0,xs,ys)==5){
    unsigned char x[16]={0},y[16]={0},r0[16]={0},r1[16]={0},env[32]={0};
    hex(xs,x,10);hex(ys,y,10);
    uint16_t cw=fcw; unsigned char env0[32]={0}; env0[0]=fcw&0xff;env0[1]=fcw>>8;env0[4]=sw0&0xff;env0[5]=sw0>>8;env0[8]=0xff;env0[9]=0xff;
    int nres=1;
    if(!strcmp(op,"fsin")){__asm__ volatile("fninit\n fldenv %[env0]\n fldt %[x]\n fsin\n fnstenv %[env]\n fstpt %[r0]\n fwait\n":[env]"=m"(env),[r0]"=m"(r0):[env0]"m"(env0),[x]"m"(x):"memory","st","st(1)");}
    else if(!strcmp(op,"fcos")){__asm__ volatile("fninit\n fldenv %[env0]\n fldt %[x]\n fcos\n fnstenv %[env]\n fstpt %[r0]\n fwait\n":[env]"=m"(env),[r0]"=m"(r0):[env0]"m"(env0),[x]"m"(x):"memory","st","st(1)");}
    else if(!strcmp(op,"f2xm1")){__asm__ volatile("fninit\n fldenv %[env0]\n fldt %[x]\n f2xm1\n fnstenv %[env]\n fstpt %[r0]\n fwait\n":[env]"=m"(env),[r0]"=m"(r0):[env0]"m"(env0),[x]"m"(x):"memory","st","st(1)");}
    else if(!strcmp(op,"fsincos")){nres=2;__asm__ volatile("fninit\n fldenv %[env0]\n fldt %[x]\n fsincos\n fnstenv %[env]\n fstpt %[r0]\n fstpt %[r1]\n fwait\n":[env]"=m"(env),[r0]"=m"(r0),[r1]"=m"(r1):[env0]"m"(env0),[x]"m"(x):"memory","st","st(1)");}
    else if(!strcmp(op,"fptan")){nres=2;__asm__ volatile("fninit\n fldenv %[env0]\n fldt %[x]\n fptan\n fnstenv %[env]\n fstpt %[r0]\n fstpt %[r1]\n fwait\n":[env]"=m"(env),[r0]"=m"(r0),[r1]"=m"(r1):[env0]"m"(env0),[x]"m"(x):"memory","st","st(1)");}
    else if(!strcmp(op,"fxtract")){nres=2;__asm__ volatile("fninit\n fldenv %[env0]\n fldt %[x]\n fxtract\n fnstenv %[env]\n fstpt %[r0]\n fstpt %[r1]\n fwait\n":[env]"=m"(env),[r0]"=m"(r0),[r1]"=m"(r1):[env0]"m"(env0),[x]"m"(x):"memory","st","st(1)");}
    else if(!strcmp(op,"fpatan")){__asm__ volatile("fninit\n fldenv %[env0]\n fldt %[y]\n fldt %[x]\n fpatan\n fnstenv %[env]\n fstpt %[r0]\n fwait\n":[env]"=m"(env),[r0]"=m"(r0):[env0]"m"(env0),[x]"m"(x),[y]"m"(y):"memory","st","st(1)");}
    else if(!strcmp(op,"fyl2x")){__asm__ volatile("fninit\n fldenv %[env0]\n fldt %[y]\n fldt %[x]\n fyl2x\n fnstenv %[env]\n fstpt %[r0]\n fwait\n":[env]"=m"(env),[r0]"=m"(r0):[env0]"m"(env0),[x]"m"(x),[y]"m"(y):"memory","st","st(1)");}
    else if(!strcmp(op,"fyl2xp1")){__asm__ volatile("fninit\n fldenv %[env0]\n fldt %[y]\n fldt %[x]\n fyl2xp1\n fnstenv %[env]\n fstpt %[r0]\n fwait\n":[env]"=m"(env),[r0]"=m"(r0):[env0]"m"(env0),[x]"m"(x),[y]"m"(y):"memory","st","st(1)");}
    else if(!strcmp(op,"fscale")){nres=2;__asm__ volatile("fninit\n fldenv %[env0]\n fldt %[y]\n fldt %[x]\n fscale\n fnstenv %[env]\n fstpt %[r0]\n fstpt %[r1]\n fwait\n":[env]"=m"(env),[r0]"=m"(r0),[r1]"=m"(r1):[env0]"m"(env0),[x]"m"(x),[y]"m"(y):"memory","st","st(1)");}
    else {continue;}
    uint16_t sw=env[4]|(env[5]<<8), tw=env[8]|(env[9]<<8);
    printf("%s %04x %04x %s %s ",op,fcw,sw0,xs,ys);ph(r0,10);printf(" ");if(nres==2)ph(r1,10);else printf("-");printf(" %04x %04x\n",sw,tw);
  }
  return 0;}

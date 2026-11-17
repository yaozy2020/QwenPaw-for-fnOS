import{c0 as O,aw as j,bf as l,b as S,F as w,c1 as I}from"./index-vfImF6cM.js";import{r as s,j as a,a as _}from"./react-vendor-Bso2UVDV.js";var F={icon:{tag:"svg",attrs:{viewBox:"64 64 896 896",focusable:"false"},children:[{tag:"path",attrs:{d:"M512 972.8c254.5 0 460.8-206.3 460.8-460.8S766.5 51.2 512 51.2 51.2 257.5 51.2 512 257.5 972.8 512 972.8zm357.04-602.15Q896 438.75 896 512t-26.96 141.35q-29.28 73.94-85.51 130.18a380.12 380.12 0 01-130.18 85.51Q585.25 896 512 896t-141.35-26.96q-73.94-29.28-130.18-85.51-56.23-56.24-85.51-130.18Q128 585.25 128 512t26.96-141.35q29.28-73.94 85.51-130.18 56.24-56.23 130.18-85.51C416.05 136.99 463.17 128 512 128s95.95 8.99 141.35 26.96q73.94 29.28 130.18 85.51c37.49 37.5 66 80.88 85.51 130.18zM569.6 262.4a57.6 57.6 0 11-115.2 0 57.6 57.6 0 01115.2 0zm30.72 433.92h-38.4V416c0-5.71-1.11-11.2-3.33-16.45a41.9 41.9 0 00-9.04-13.42 41.9 41.9 0 00-13.42-9.04 41.82 41.82 0 00-16.45-3.33h-42.24a38.4 38.4 0 000 76.8h7.68v245.76h-38.4a38.4 38.4 0 000 76.8h153.6a38.4 38.4 0 000-76.8z"}}]},name:"info-circle",theme:"outlined"};function b(){return b=Object.assign?Object.assign.bind():function(e){for(var r=1;r<arguments.length;r++){var t=arguments[r];for(var n in t)Object.prototype.hasOwnProperty.call(t,n)&&(e[n]=t[n])}return e},b.apply(this,arguments)}const R=(e,r)=>s.createElement(O,b({},e,{ref:r,icon:F})),E=s.forwardRef(R);var g;function k(e,r){return r||(r=e.slice(0)),Object.freeze(Object.defineProperties(e,{raw:{value:Object.freeze(r)}}))}var q=j(g||(g=k([`
.`,`-form {
  .`,`-required-mark {
    color: var(--`,`-color-error);
    line-height: 1;
    margin-top: 0;
    font-size: 16px;
  }
  
  .`,"-form-item .",`-form-item-label {
    text-align: left;
  }
}

.`,"-form-item .",`-form-item-label > label {
  font-weight: 500;
  white-space: normal;
  font-size: 13px;
  line-height: 20px;
  color: var(--`,`-color-text);
  gap: 4px;
}

.`,`-form-label-margin-small {
  .`,`-form-item-label > label {
    margin-right: 8px;
  }
}

.`,`-col {
  min-height: unset;
}
`])),function(e){return e.sparkPrefix},function(e){return e.sparkPrefix},function(e){return e.antPrefix},function(e){return e.antPrefix},function(e){return e.antPrefix},function(e){return e.antPrefix},function(e){return e.antPrefix},function(e){return e.antPrefix},function(e){return e.sparkPrefix},function(e){return e.antPrefix},function(e){return e.antPrefix}),C=["labelMarginRight"],z=["tooltip"];function c(e){"@babel/helpers - typeof";return c=typeof Symbol=="function"&&typeof Symbol.iterator=="symbol"?function(r){return typeof r}:function(r){return r&&typeof Symbol=="function"&&r.constructor===Symbol&&r!==Symbol.prototype?"symbol":typeof r},c(e)}function v(e,r){var t=Object.keys(e);if(Object.getOwnPropertySymbols){var n=Object.getOwnPropertySymbols(e);r&&(n=n.filter(function(o){return Object.getOwnPropertyDescriptor(e,o).enumerable})),t.push.apply(t,n)}return t}function u(e){for(var r=1;r<arguments.length;r++){var t=arguments[r]!=null?arguments[r]:{};r%2?v(Object(t),!0).forEach(function(n){M(e,n,t[n])}):Object.getOwnPropertyDescriptors?Object.defineProperties(e,Object.getOwnPropertyDescriptors(t)):v(Object(t)).forEach(function(n){Object.defineProperty(e,n,Object.getOwnPropertyDescriptor(t,n))})}return e}function M(e,r,t){return r=L(r),r in e?Object.defineProperty(e,r,{value:t,enumerable:!0,configurable:!0,writable:!0}):e[r]=t,e}function L(e){var r=N(e,"string");return c(r)=="symbol"?r:String(r)}function N(e,r){if(c(e)!="object"||!e)return e;var t=e[Symbol.toPrimitive];if(t!==void 0){var n=t.call(e,r);if(c(n)!="object")return n;throw new TypeError("@@toPrimitive must return a primitive value.")}return(r==="string"?String:Number)(e)}function y(e,r){if(e==null)return{};var t=D(e,r),n,o;if(Object.getOwnPropertySymbols){var i=Object.getOwnPropertySymbols(e);for(o=0;o<i.length;o++)n=i[o],!(r.indexOf(n)>=0)&&Object.prototype.propertyIsEnumerable.call(e,n)&&(t[n]=e[n])}return t}function D(e,r){if(e==null)return{};var t={},n=Object.keys(e),o,i;for(i=0;i<n.length;i++)o=n[i],!(r.indexOf(o)>=0)&&(t[o]=e[o]);return t}var T=s.forwardRef(function(e,r){var t=e.labelMarginRight,n=t===void 0?void 0:t,o=y(e,C),i=s.useRef(null),m=w(),p=m.sparkPrefix,h=q();return s.useImperativeHandle(r,function(){return i.current}),a.jsxs(a.Fragment,{children:[a.jsx(h,{}),a.jsx(l,u(u({},o),{},{className:S(e.className,"".concat(p,"-form"),n==="small"&&"".concat(p,"-form-label-margin-small")),ref:i,requiredMark:function(x,P){return a.jsxs(a.Fragment,{children:[x,P.required&&a.jsx("span",{className:"".concat(p,"-required-mark"),children:"*"})]})}}))]})}),W=function(r){var t=r.tooltip,n=y(r,z),o=a.jsx(E,{});function i(){if(t){var m=c(t)==="object"&&!_.isValidElement(t)?u(u({},t),{},{icon:t.icon||o}):{title:t,icon:o};return m}}return a.jsx(l.Item,u(u({},n),{},{tooltip:i(),labelCol:r.labelCol||(r.layout==="vertical"?{flex:"unset"}:void 0)}))},d=W;d.useStatus=I.useStatus;var f=T;f.Item=d;f.List=l.List;f.ErrorList=l.ErrorList;f.Provider=l.Provider;f.useForm=l.useForm;f.useFormInstance=l.useFormInstance;f.useWatch=l.useWatch;export{E as R,f as S};
